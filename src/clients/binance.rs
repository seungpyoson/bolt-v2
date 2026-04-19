use nautilus_binance::{config::BinanceDataClientConfig, factories::BinanceDataClientFactory};

use crate::{
    clients::ReferenceDataClientParts, config::BinanceSharedConfig, secrets::ResolvedBinanceSecrets,
};

pub fn build_reference_data_client_with_secrets(
    shared: &BinanceSharedConfig,
    secrets: ResolvedBinanceSecrets,
) -> ReferenceDataClientParts {
    (
        Box::new(BinanceDataClientFactory::new()),
        Box::new(BinanceDataClientConfig {
            product_types: shared.product_types.clone(),
            environment: shared.environment,
            base_url_http: shared.base_url_http.clone(),
            base_url_ws: shared.base_url_ws.clone(),
            api_key: Some(secrets.api_key),
            api_secret: Some(secrets.api_secret),
            instrument_status_poll_secs: shared.instrument_status_poll_secs,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_reference_data_client_with_secrets_populates_auth_fields() {
        let shared = BinanceSharedConfig {
            region: "eu-west-1".to_string(),
            api_key: "/bolt/binance/api-key".to_string(),
            api_secret: "/bolt/binance/api-secret".to_string(),
            environment: Default::default(),
            product_types: vec![nautilus_binance::common::enums::BinanceProductType::Spot],
            instrument_status_poll_secs: 3600,
            base_url_http: None,
            base_url_ws: None,
        };
        let secrets = ResolvedBinanceSecrets {
            api_key: "api-key".to_string(),
            api_secret: "api-secret".to_string(),
        };
        let (_, config) = build_reference_data_client_with_secrets(&shared, secrets.clone());
        let config = config
            .as_any()
            .downcast_ref::<BinanceDataClientConfig>()
            .expect("binance reference builder should produce BinanceDataClientConfig");

        assert_eq!(config.api_key.as_deref(), Some(secrets.api_key.as_str()));
        assert_eq!(
            config.api_secret.as_deref(),
            Some(secrets.api_secret.as_str())
        );
        assert_eq!(config.product_types, shared.product_types);
        assert_eq!(config.environment, shared.environment);
        assert_eq!(config.instrument_status_poll_secs, 3600);
    }
}
