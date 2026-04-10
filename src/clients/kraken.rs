use nautilus_kraken::{config::KrakenDataClientConfig, factories::KrakenDataClientFactory};

use crate::clients::ReferenceDataClientParts;

pub fn build_reference_data_client() -> ReferenceDataClientParts {
    (
        Box::new(KrakenDataClientFactory::new()),
        Box::new(KrakenDataClientConfig::default()),
    )
}
