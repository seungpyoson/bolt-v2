//! Strategy-archetype validation for the existing `EthChainlinkTaker`.

use rust_decimal::Decimal;

use crate::{
    bolt_v3_config::BoltV3StrategyConfig,
    bolt_v3_market_families::updown::TargetBlock,
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
    validate_period_matches_target(context, strategy, &raw, &mut errors);
    errors
}

fn validate_period_matches_target(
    context: &str,
    strategy: &BoltV3StrategyConfig,
    raw: &toml::Value,
    errors: &mut Vec<String>,
) {
    let Ok(target) = strategy.target.clone().try_into::<TargetBlock>() else {
        return;
    };
    let Some(period_duration_secs) = raw
        .as_table()
        .and_then(|table| table.get("period_duration_secs"))
        .and_then(toml::Value::as_integer)
    else {
        return;
    };

    if target.cadence_seconds != period_duration_secs {
        errors.push(format!(
            "{context}: target.cadence_seconds must match parameters.period_duration_secs for `eth_chainlink_taker` \
             (target.cadence_seconds={}, parameters.period_duration_secs={period_duration_secs})",
            target.cadence_seconds
        ));
    }
}

fn format_strategy_error(error: ValidationError) -> String {
    format!("parameters bridge: {error}")
}
