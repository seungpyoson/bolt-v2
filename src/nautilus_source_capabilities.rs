//! Build-generated capability facts for the exact official NautilusTrader pin.
//!
//! These facts are generated from governed CI policy and are not exposed in
//! operator TOML. The selected source supports both required Binance facts, so
//! production uses one direct path without runtime capability branches or
//! fallback providers.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NautilusSourceCapabilityRegistry {
    pub revision: &'static str,
    pub binance_spot_sbe_schema_3_5: bool,
    pub binance_adapter_receive_timestamps: bool,
}

include!(concat!(env!("OUT_DIR"), "/nautilus_source_capabilities.rs"));
