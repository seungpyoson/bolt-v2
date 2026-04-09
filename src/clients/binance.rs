use nautilus_binance::{
    config::BinanceDataClientConfig, factories::BinanceDataClientFactory,
};

use crate::clients::ReferenceDataClientParts;

pub fn build_reference_data_client() -> ReferenceDataClientParts {
    (
        Box::new(BinanceDataClientFactory::new()),
        Box::new(BinanceDataClientConfig::default()),
    )
}
