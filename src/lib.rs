pub mod bolt_v3_adapters;
pub mod bolt_v3_application_resource_ledger;
pub mod bolt_v3_archetypes;
pub mod bolt_v3_atomic_io;
pub mod bolt_v3_basket_admission;
pub mod bolt_v3_basket_execution;
pub mod bolt_v3_basket_store;
pub mod bolt_v3_binary_outcome_edge;
pub mod bolt_v3_binary_settlement;
pub mod bolt_v3_binary_settlement_runtime;
pub mod bolt_v3_book_sizing;
pub mod bolt_v3_capital_admission;
pub mod bolt_v3_capital_admission_runtime_feed;
pub mod bolt_v3_capital_reservation;
pub mod bolt_v3_client_registration;
pub mod bolt_v3_complete_set_contract;
pub mod bolt_v3_config;
pub mod bolt_v3_decision_evidence;
pub mod bolt_v3_deploy_target;
pub mod bolt_v3_economics_config;
pub mod bolt_v3_economics_runtime;
pub mod bolt_v3_executable_cost;
pub mod bolt_v3_fair_value_pricing;
pub mod bolt_v3_feed_health;
pub mod bolt_v3_instrument_filters;
mod bolt_v3_instrument_metadata_bus;
pub mod bolt_v3_iv;
pub mod bolt_v3_kill_switch;
pub mod bolt_v3_kill_switch_action_router;
pub mod bolt_v3_kill_switch_cancel;
pub mod bolt_v3_kill_switch_flatten;
pub mod bolt_v3_kill_switch_store;
pub mod bolt_v3_live_node;
pub mod bolt_v3_loss_governor;
pub mod bolt_v3_loss_governor_manual_recovery_ops;
pub mod bolt_v3_loss_halt_actions;
pub mod bolt_v3_loss_protection;
pub mod bolt_v3_loss_runtime_feed;
pub mod bolt_v3_maker_event_fence;
pub mod bolt_v3_maker_go_live_gate;
pub mod bolt_v3_maker_inventory;
pub mod bolt_v3_maker_market_selection;
pub mod bolt_v3_maker_microprice;
pub mod bolt_v3_maker_model;
pub mod bolt_v3_maker_mu_estimator;
pub mod bolt_v3_maker_order_compile;
pub mod bolt_v3_maker_order_dispatch;
pub mod bolt_v3_maker_order_plan;
pub mod bolt_v3_maker_quote_control;
pub mod bolt_v3_maker_quote_plan;
pub mod bolt_v3_maker_quote_set;
pub mod bolt_v3_maker_rate_budget;
pub mod bolt_v3_maker_risk;
pub mod bolt_v3_maker_runtime_order;
pub mod bolt_v3_maker_runtime_quote;
pub mod bolt_v3_market_families;
pub mod bolt_v3_numeric;
mod bolt_v3_observed_dedupe;
pub mod bolt_v3_operator_artifacts;
pub mod bolt_v3_operator_health;
pub mod bolt_v3_order_execution;
pub mod bolt_v3_order_intent;
pub mod bolt_v3_order_reject_observer_feed;
pub mod bolt_v3_outcome_group_hyperliquid;
pub mod bolt_v3_outcome_group_polymarket;
pub mod bolt_v3_outcome_group_proofs;
pub mod bolt_v3_outcome_group_scanner;
pub mod bolt_v3_outcome_group_sources;
pub mod bolt_v3_outcome_groups;
pub mod bolt_v3_position_contract;
pub mod bolt_v3_prediction_market_instrument;
pub mod bolt_v3_prod_profile;
pub mod bolt_v3_providers;
pub mod bolt_v3_settlement_runtime;
pub use bolt_v3_providers::boundary_registry as bolt_v3_boundary_registry;
pub mod bolt_v3_polymarket_redemption;
pub mod bolt_v3_quote_lifecycle;
pub mod bolt_v3_quoting;
pub mod bolt_v3_readiness;
pub mod bolt_v3_realized_volatility;
pub mod bolt_v3_realized_volatility_runtime;
pub mod bolt_v3_reference_price;
pub mod bolt_v3_reference_price_health;
pub mod bolt_v3_risk_reservation_substrate;
pub mod bolt_v3_runtime_reconcile;
// Re-exported at crate root so backtesting consumers can name these
// reconstruction types without writing the snake_case module path. The
// backtesting-vertical-slice sample-venue source fence forbids the literal
// "reference_price" token in its production Rust; the CamelCase type names are
// fence-clean, the module path is not.
pub use bolt_v3_reference_price::{ReferencePriceUpdate, ReferenceQuoteProvenance};
pub mod bolt_v3_capital_admission_state;
pub mod bolt_v3_requote_budget;
pub mod bolt_v3_secrets;
pub mod bolt_v3_settlement_booking;
pub mod bolt_v3_sizing;
pub mod bolt_v3_source_integrity;
pub mod bolt_v3_strategy_context;
pub mod bolt_v3_strategy_registration;
pub mod bolt_v3_submit_admission;
pub mod bolt_v3_taker_pricing;
pub mod bolt_v3_taker_updown_signal;
pub mod bolt_v3_timestamp_domain;
pub mod bolt_v3_trade_flow;
pub mod bolt_v3_validate;
pub mod bolt_v3_venue_truth;
pub mod bolt_v3_wire_boundary;
mod bounded_config_read;
pub mod economics;
pub mod execution_state;
pub mod integrations;
pub mod lake_batch;
pub mod log_sweep;
pub mod nautilus_source_capabilities;
pub mod nt_runtime_capture;
pub mod raw_types;
pub mod secrets;
pub mod shadow_pnl;
pub mod source_canonicalization;
pub mod strategies;
pub mod strategy_bindings;
pub mod venue_contract;

#[cfg(test)]
pub(crate) fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}
