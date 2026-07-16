//! Build-generated, immutable capabilities of the exact NautilusTrader source pin.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NautilusSourceCapabilities {
    pub git: &'static str,
    pub revision: &'static str,
    pub binance_spot_sbe_schema_3_5: bool,
    pub binance_spot_sbe_adapter_receive_clock: bool,
    pub binance_spot_sbe_new_risk_quorum: bool,
}

include!(concat!(env!("OUT_DIR"), "/nautilus_source_capabilities.rs"));
