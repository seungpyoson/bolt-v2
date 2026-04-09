use nautilus_deribit::{
    config::DeribitDataClientConfig, factories::DeribitDataClientFactory,
};

use crate::clients::ReferenceDataClientParts;

pub fn build_reference_data_client() -> ReferenceDataClientParts {
    (
        Box::new(DeribitDataClientFactory::new()),
        Box::new(DeribitDataClientConfig::default()),
    )
}
