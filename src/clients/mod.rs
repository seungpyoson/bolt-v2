use nautilus_system::factories::{ClientConfig, DataClientFactory};

pub type ReferenceDataClientParts = (Box<dyn DataClientFactory>, Box<dyn ClientConfig>);

pub mod binance;
pub mod bybit;
pub mod deribit;
pub mod hyperliquid;
pub mod kraken;
pub mod okx;
pub mod polymarket;
