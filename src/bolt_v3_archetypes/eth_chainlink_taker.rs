//! Strategy-archetype validation for the existing `EthChainlinkTaker`.

use rust_decimal::Decimal;

use crate::{
    bolt_v3_config::BoltV3StrategyConfig,
    bolt_v3_strategy_registration::eth_chainlink_taker::legacy_config_from_strategy,
    strategies::{
        eth_chainlink_taker::ETH_CHAINLINK_TAKER_KIND,
        eth_chainlink_taker::EthChainlinkTakerBuilder, registry::StrategyBuilder,
    },
    validate::ValidationError,
};

pub const KEY: &str = ETH_CHAINLINK_TAKER_KIND;

pub fn validate_strategy(
    context: &str,
    strategy: &BoltV3StrategyConfig,
    _default_max_notional: Option<&Decimal>,
) -> Vec<String> {
    let mut errors = Vec::new();

    let raw = match legacy_config_from_strategy(strategy) {
        Ok(raw) => raw,
        Err(error) => {
            errors.push(format!("{context}: {error}"));
            return errors;
        }
    };

    let mut strategy_errors = Vec::new();
    EthChainlinkTakerBuilder::validate_config(&raw, "parameters", &mut strategy_errors);
    errors.extend(strategy_errors.into_iter().map(format_strategy_error));
    errors
}

fn format_strategy_error(error: ValidationError) -> String {
    format!("parameters bridge: {error}")
}
