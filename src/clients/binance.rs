use nautilus_binance::{config::BinanceDataClientConfig, factories::BinanceDataClientFactory};

use crate::{
    clients::ReferenceDataClientParts,
    config::ReferenceConfig,
    secrets::{ResolvedBinanceSecrets, resolve_binance},
};

pub fn build_reference_data_client() -> ReferenceDataClientParts {
    (
        Box::new(BinanceDataClientFactory::new()),
        Box::new(BinanceDataClientConfig::default()),
    )
}

pub fn build_reference_data_client_with_reference(
    reference: &ReferenceConfig,
) -> Result<ReferenceDataClientParts, Box<dyn std::error::Error>> {
    let shared = reference.binance.as_ref().ok_or_else(|| {
        std::io::Error::other(
            "missing shared binance config for configured binance reference venues",
        )
    })?;
    let secrets = resolve_binance(&shared.region, &shared.api_key, &shared.api_secret)?;
    Ok(build_reference_data_client_with_secrets(secrets))
}

pub fn build_reference_data_client_with_secrets(
    secrets: ResolvedBinanceSecrets,
) -> ReferenceDataClientParts {
    (
        Box::new(BinanceDataClientFactory::new()),
        Box::new(BinanceDataClientConfig {
            api_key: Some(secrets.api_key),
            api_secret: Some(secrets.api_secret),
            ..Default::default()
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_reference_data_client_with_secrets_populates_auth_fields() {
        let secrets = ResolvedBinanceSecrets {
            api_key: "api-key".to_string(),
            api_secret: "api-secret".to_string(),
        };
        let (_, config) = build_reference_data_client_with_secrets(secrets.clone());
        let config = config
            .as_any()
            .downcast_ref::<BinanceDataClientConfig>()
            .expect("binance reference builder should produce BinanceDataClientConfig");

        assert_eq!(config.api_key.as_deref(), Some(secrets.api_key.as_str()));
        assert_eq!(
            config.api_secret.as_deref(),
            Some(secrets.api_secret.as_str())
        );
    }
}
