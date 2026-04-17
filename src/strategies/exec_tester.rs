use nautilus_model::{
    identifiers::{ClientId, InstrumentId, StrategyId},
    types::Quantity,
};
use nautilus_testkit::testers::{ExecTester, ExecTesterConfig};
use nautilus_trading::strategy::StrategyConfig;
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

    let config = ExecTesterConfig::builder()
        .base(StrategyConfig {
            strategy_id: Some(strategy_id),
            external_order_claims: Some(vec![instrument_id]),
            ..Default::default()
        })
        .instrument_id(instrument_id)
        .client_id(client_id)
        .order_qty(Quantity::from(cfg.order_qty.as_str()))
        .log_data(cfg.log_data)
        .use_post_only(cfg.use_post_only)
        .tob_offset_ticks(cfg.tob_offset_ticks)
        .enable_limit_sells(cfg.enable_limit_sells)
        .enable_stop_buys(cfg.enable_stop_buys)
        .enable_stop_sells(cfg.enable_stop_sells)
        .build();

    Ok(ExecTester::new(config))
}
