use std::{num::NonZeroUsize, str::FromStr};

use crate::live_config::parse_time_in_force_token;
use nautilus_model::{
    identifiers::{ClientId, InstrumentId, StrategyId},
    types::Quantity,
};
use nautilus_testkit::testers::{ExecTester, ExecTesterConfig};
use nautilus_trading::strategy::StrategyConfig;
use rust_decimal::Decimal;
use serde::Deserialize;
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
    #[serde(default = "crate::live_config::default_book_interval_ms")]
    pub book_interval_ms: u64,
    #[serde(default)]
    pub open_position_on_start_qty: Option<String>,
    #[serde(default)]
    pub open_position_time_in_force: Option<String>,
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

pub fn build_exec_tester(raw: &Value) -> Result<ExecTester, Box<dyn std::error::Error>> {
    let cfg: ExecTesterInput = raw.clone().try_into()?;

    let instrument_id = InstrumentId::from(cfg.instrument_id.as_str());
    let strategy_id = StrategyId::from(cfg.strategy_id.as_str());
    let client_id = ClientId::new(cfg.client_id);
    let book_interval_ms = NonZeroUsize::new(cfg.book_interval_ms as usize)
        .ok_or("book_interval_ms must be greater than zero")?;

    let mut config = ExecTesterConfig::builder()
        .base(StrategyConfig {
            strategy_id: Some(strategy_id),
            external_order_claims: Some(vec![instrument_id]),
            ..Default::default()
        })
        .instrument_id(instrument_id)
        .client_id(client_id)
        .order_qty(Quantity::from(cfg.order_qty.as_str()))
        .log_data(cfg.log_data)
        .subscribe_book(cfg.subscribe_book)
        .book_interval_ms(book_interval_ms)
        .use_post_only(cfg.use_post_only)
        .tob_offset_ticks(cfg.tob_offset_ticks)
        .enable_limit_sells(cfg.enable_limit_sells)
        .enable_stop_buys(cfg.enable_stop_buys)
        .enable_stop_sells(cfg.enable_stop_sells)
        .build();

    if let Some(open_position_on_start_qty) = cfg.open_position_on_start_qty {
        config.open_position_on_start_qty =
            Some(Decimal::from_str(open_position_on_start_qty.as_str())?);
    }

    if let Some(open_position_time_in_force) = cfg.open_position_time_in_force {
        config.open_position_time_in_force = parse_time_in_force_token(
            open_position_time_in_force.as_str(),
        )
        .map_err(|e| format!("invalid open_position_time_in_force: {e}"))?
        ;
    }

    Ok(ExecTester::new(config))
}
