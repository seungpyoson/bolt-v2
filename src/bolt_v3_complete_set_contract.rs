//! Shared complete-set submit contract.
//!
//! This module owns the strategy-independent submit-mode vocabulary and its
//! NautilusTrader order-template projection. Shared outcome-group code may use
//! these types without importing the complete-set strategy layer.

use nautilus_model::enums::{OrderType, TimeInForce};
use serde::Deserialize;

use crate::bolt_v3_order_intent::{NtOrderTemplateConfig, check_nt_order_template_config};

pub const COMPLETE_SET_ARBITRAGE_KEY: &str = "complete_set_arbitrage";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompleteSetSubmitMode {
    Ioc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteSetSubmitModeContract {
    pub submit_mode: CompleteSetSubmitMode,
    pub order_template: NtOrderTemplateConfig,
    pub nt_template_errors: Vec<String>,
}

/// Shared projection of a complete-set strategy's raw `[parameters]` table.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompleteSetArbitrageParametersBlock {
    pub runtime: CompleteSetArbitrageRuntimeBlock,
}

/// Shared projection consumed by both outcome-group validation and strategy
/// config translation.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompleteSetArbitrageRuntimeBlock {
    pub min_edge_bps: i64,
    pub max_basket_notional: String,
    pub max_open_baskets: u32,
    pub submit_mode: String,
    pub vwap_depth_limit_bps: u64,
    pub slippage_buffer_bps: u64,
    pub max_repair_attempts: u32,
    pub max_unwind_attempts: u32,
}

pub fn supported_submit_modes() -> Vec<CompleteSetSubmitMode> {
    vec![CompleteSetSubmitMode::Ioc]
}

pub fn submit_mode_contract(mode: CompleteSetSubmitMode) -> CompleteSetSubmitModeContract {
    let order_template = match mode {
        CompleteSetSubmitMode::Ioc => NtOrderTemplateConfig {
            order_type: OrderType::Market,
            time_in_force: TimeInForce::Ioc,
            expire_time_unix_nanos: None,
            trigger_price: None,
            activation_price: None,
            trigger_type: None,
            trigger_instrument_id: None,
            trailing_offset: None,
            trailing_offset_type: None,
            is_post_only: false,
            is_reduce_only: false,
            is_quote_quantity: false,
        },
    };
    let nt_template_errors = check_nt_order_template_config(
        COMPLETE_SET_ARBITRAGE_KEY,
        "parameters.runtime.submit_mode.ioc.order_template",
        &order_template,
    );
    CompleteSetSubmitModeContract {
        submit_mode: mode,
        order_template,
        nt_template_errors,
    }
}

impl CompleteSetSubmitMode {
    pub fn from_config(value: &str) -> Option<Self> {
        match value {
            "ioc" => Some(Self::Ioc),
            _ => None,
        }
    }

    pub fn as_config(self) -> &'static str {
        match self {
            Self::Ioc => "ioc",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_mode_config_conversion_is_exact() {
        assert_eq!(
            CompleteSetSubmitMode::from_config("ioc"),
            Some(CompleteSetSubmitMode::Ioc)
        );
        assert_eq!(CompleteSetSubmitMode::Ioc.as_config(), "ioc");
        assert_eq!(CompleteSetSubmitMode::from_config("IOC"), None);
        assert_eq!(CompleteSetSubmitMode::from_config("scan_all"), None);
    }

    #[test]
    fn supported_mode_projects_to_the_pinned_nt_template() {
        assert_eq!(supported_submit_modes(), vec![CompleteSetSubmitMode::Ioc]);
        let contract = submit_mode_contract(CompleteSetSubmitMode::Ioc);
        assert_eq!(contract.submit_mode, CompleteSetSubmitMode::Ioc);
        assert_eq!(contract.order_template.order_type, OrderType::Market);
        assert_eq!(contract.order_template.time_in_force, TimeInForce::Ioc);
        assert!(contract.nt_template_errors.is_empty());
    }

    #[test]
    fn shared_parameter_projection_rejects_unknown_fields() {
        let parameters: toml::Value = toml::from_str(
            r#"
[runtime]
min_edge_bps = 1
max_basket_notional = "10"
max_open_baskets = 1
submit_mode = "ioc"
vwap_depth_limit_bps = 100
slippage_buffer_bps = 10
max_repair_attempts = 1
max_unwind_attempts = 1
unknown = true
"#,
        )
        .expect("fixture TOML parses");

        assert!(
            parameters
                .try_into::<CompleteSetArbitrageParametersBlock>()
                .is_err(),
            "shared projection must preserve deny_unknown_fields"
        );
    }
}
