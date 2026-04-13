use std::{cell::RefCell, rc::Rc};

use anyhow::anyhow;
use nautilus_common::component::Component;
use nautilus_model::{
    identifiers::{ClientId, InstrumentId, StrategyId},
    types::Quantity,
};
use nautilus_system::trader::Trader;
use nautilus_testkit::testers::{ExecTester, ExecTesterConfig};
use nautilus_trading::strategy::StrategyConfig;
use serde::Deserialize;
use toml::Value;

use super::registry::{BoxedStrategy, StrategyBuildContext, StrategyBuilder};
use crate::validate::{
    ValidationError, check_instrument_id, check_nt_ascii, check_strictly_positive_qty, push_error,
};

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

pub struct ExecTesterBuilder;

impl StrategyBuilder for ExecTesterBuilder {
    fn kind() -> &'static str {
        "exec_tester"
    }

    fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>) {
        match raw.get("client_id") {
            None => push_error(
                errors,
                &format!("{field_prefix}.client_id"),
                "missing_client_id",
                "is missing required string field".to_string(),
            ),
            Some(value) => {
                let field = format!("{field_prefix}.client_id");
                if let Some(client_id) = value.as_str() {
                    check_nt_ascii(errors, &field, client_id);
                } else {
                    push_error(
                        errors,
                        &field,
                        "wrong_type",
                        format!("must be a string, got {} value", value.type_str()),
                    );
                }
            }
        }

        match raw.get("instrument_id") {
            None => push_error(
                errors,
                &format!("{field_prefix}.instrument_id"),
                "missing_instrument_id",
                "is missing required string field".to_string(),
            ),
            Some(value) => {
                let field = format!("{field_prefix}.instrument_id");
                if let Some(instrument_id) = value.as_str() {
                    check_instrument_id(errors, &field, instrument_id);
                } else {
                    push_error(
                        errors,
                        &field,
                        "wrong_type",
                        format!("must be a string, got {} value", value.type_str()),
                    );
                }
            }
        }

        match raw.get("order_qty") {
            None => push_error(
                errors,
                &format!("{field_prefix}.order_qty"),
                "missing_order_qty",
                "is missing required string field".to_string(),
            ),
            Some(value) => {
                let field = format!("{field_prefix}.order_qty");
                if let Some(order_qty) = value.as_str() {
                    check_strictly_positive_qty(errors, &field, order_qty);
                } else {
                    push_error(
                        errors,
                        &field,
                        "wrong_type",
                        format!("must be a string, got {} value", value.type_str()),
                    );
                }
            }
        }
    }

    fn build(raw: &Value, _context: &StrategyBuildContext) -> anyhow::Result<BoxedStrategy> {
        let strategy = build_exec_tester(raw).map_err(|error| anyhow!(error.to_string()))?;
        Ok(Box::new(strategy))
    }

    fn register(
        raw: &Value,
        _context: &StrategyBuildContext,
        trader: &Rc<RefCell<Trader>>,
    ) -> anyhow::Result<StrategyId> {
        let strategy = build_exec_tester(raw).map_err(|error| anyhow!(error.to_string()))?;
        let strategy_id = StrategyId::from(strategy.component_id().inner().as_str());
        trader.borrow_mut().add_strategy(strategy)?;
        Ok(strategy_id)
    }
}
