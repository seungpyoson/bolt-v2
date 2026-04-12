use std::num::NonZeroUsize;

use nautilus_model::{
    enums::TimeInForce,
    identifiers::{ClientId, InstrumentId, StrategyId},
    types::Quantity,
};
use nautilus_testkit::testers::{ExecTester, ExecTesterConfig};
use nautilus_trading::strategy::StrategyConfig;
use serde::Deserialize;
use rust_decimal::Decimal;
use toml::Value;

#[derive(Debug, Deserialize)]
pub struct ExecTesterInput {
    pub strategy_id: String,
    pub instrument_id: String,
    pub client_id: String,
    pub order_qty: String,
    #[serde(default)]
    pub log_data: bool,
    #[serde(default)]
    pub subscribe_book: bool,
    pub book_interval_ms: Option<usize>,
    pub open_position_on_start_qty: Option<String>,
    pub open_position_time_in_force: Option<TimeInForce>,
    #[serde(default)]
    pub tob_offset_ticks: u64,
    #[serde(default)]
    pub use_post_only: bool,
    #[serde(default)]
    pub enable_limit_sells: bool,
    #[serde(default)]
    pub enable_stop_buys: bool,
    #[serde(default)]
    pub enable_stop_sells: bool,
}

fn build_exec_tester_config(raw: &Value) -> Result<ExecTesterConfig, Box<dyn std::error::Error>> {
    let cfg: ExecTesterInput = raw.clone().try_into()?;

    let instrument_id = InstrumentId::from(cfg.instrument_id.as_str());
    let strategy_id = StrategyId::from(cfg.strategy_id.as_str());
    let client_id = ClientId::new(cfg.client_id);

    let mut config = ExecTesterConfig::new(
        strategy_id,
        instrument_id,
        client_id,
        Quantity::from(cfg.order_qty.as_str()),
    );
    config.base = StrategyConfig {
        strategy_id: Some(strategy_id),
        external_order_claims: Some(vec![instrument_id]),
        ..Default::default()
    };
    config.log_data = cfg.log_data;
    config.subscribe_book = cfg.subscribe_book;
    config.use_post_only = cfg.use_post_only;
    config.tob_offset_ticks = cfg.tob_offset_ticks;
    config.enable_limit_sells = cfg.enable_limit_sells;
    config.enable_stop_buys = cfg.enable_stop_buys;
    config.enable_stop_sells = cfg.enable_stop_sells;

    if let Some(book_interval_ms) = cfg.book_interval_ms {
        config.book_interval_ms =
            NonZeroUsize::new(book_interval_ms).ok_or("book_interval_ms must be greater than zero")?;
    }

    if let Some(open_position_on_start_qty) = cfg.open_position_on_start_qty.as_deref() {
        config.open_position_on_start_qty = Some(open_position_on_start_qty.parse::<Decimal>()?);
    }

    if let Some(open_position_time_in_force) = cfg.open_position_time_in_force {
        config.open_position_time_in_force = open_position_time_in_force;
    }

    Ok(config)
}

pub fn build_exec_tester(raw: &Value) -> Result<ExecTester, Box<dyn std::error::Error>> {
    Ok(ExecTester::new(build_exec_tester_config(raw)?))
}

#[cfg(test)]
mod tests {
    use nautilus_model::enums::TimeInForce;

    use super::build_exec_tester_config;

    #[test]
    fn build_exec_tester_surfaces_book_and_fok_entry_knobs() {
        let raw = toml::toml! {
            strategy_id = "EXEC_TESTER-001"
            instrument_id = "TOKEN.POLYMARKET"
            client_id = "TEST"
            order_qty = "5"
            log_data = false
            subscribe_book = true
            book_interval_ms = 250
            open_position_on_start_qty = "5"
            open_position_time_in_force = "Fok"
            tob_offset_ticks = 5
            use_post_only = false
            enable_limit_sells = false
            enable_stop_buys = false
            enable_stop_sells = false
        }
        .into();

        let config =
            build_exec_tester_config(&raw).expect("extended runtime seam config should parse");

        assert!(config.subscribe_book);
        assert_eq!(config.book_interval_ms.get(), 250);
        assert_eq!(
            config.open_position_on_start_qty.map(|qty| qty.to_string()),
            Some("5".to_string())
        );
        assert_eq!(config.open_position_time_in_force, TimeInForce::Fok);
    }
}
