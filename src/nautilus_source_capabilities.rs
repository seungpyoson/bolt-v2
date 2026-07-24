//! Build-generated capability facts for the exact official NautilusTrader pin.
//!
//! These facts are generated from governed CI policy and are not exposed in
//! operator TOML. The selected source supports the required Binance market-data
//! facts and Polymarket reconciliation fail-closed facts, so production uses
//! direct NT paths without runtime capability branches or fallback providers.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NautilusSourceCapabilityRegistry {
    pub revision: &'static str,
    pub binance_spot_sbe_schema_3_5: bool,
    pub binance_adapter_receive_timestamps: bool,
    pub polymarket_reconciliation_rejects_unmapped_open_orders: bool,
    pub polymarket_reconciliation_rejects_unmapped_confirmed_fills: bool,
    pub polymarket_reconciliation_rejects_unrepresentable_positions: bool,
}

include!(concat!(env!("OUT_DIR"), "/nautilus_source_capabilities.rs"));
