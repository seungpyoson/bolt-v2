use nautilus_bybit::{config::BybitDataClientConfig, factories::BybitDataClientFactory};

use crate::clients::ReferenceDataClientParts;

pub fn build_reference_data_client() -> ReferenceDataClientParts {
    (
        Box::new(BybitDataClientFactory::new()),
        Box::new(BybitDataClientConfig::default()),
    )
}
