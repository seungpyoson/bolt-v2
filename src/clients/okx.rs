use nautilus_okx::{config::OKXDataClientConfig, factories::OKXDataClientFactory};

use crate::clients::ReferenceDataClientParts;

pub fn build_reference_data_client() -> ReferenceDataClientParts {
    (
        Box::new(OKXDataClientFactory::new()),
        Box::new(OKXDataClientConfig::default()),
    )
}
