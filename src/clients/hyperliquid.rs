use nautilus_hyperliquid::{
    config::HyperliquidDataClientConfig, factories::HyperliquidDataClientFactory,
};

use crate::clients::ReferenceDataClientParts;

pub fn build_reference_data_client() -> ReferenceDataClientParts {
    (
        Box::new(HyperliquidDataClientFactory::new()),
        Box::new(HyperliquidDataClientConfig::default()),
    )
}
