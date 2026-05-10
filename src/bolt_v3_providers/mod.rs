//! Per-provider binding root for bolt-v3 client config
//! block shapes and per-instance startup-validation policy.
//!
//! Core config in `crate::bolt_v3_config` owns the root and strategy
//! envelopes plus raw venue keys. Concrete venue key
//! literals and `[clients.<name>.{data,execution,secrets}]`
//! block shapes live in
//! per-provider binding modules under this root.
//!
//! This module also owns the family-agnostic dispatch surface that
//! core startup validation in `crate::bolt_v3_validate` calls into:
//! every `[clients.<id>]` block is routed here, the
//! venue key is read once, and the matching per-provider
//! validator owns the rest of the structural instance-shape rules.
//! Provider-neutral helpers used by more than one provider validator
//! (today: `crate::bolt_v3_validate::validate_ssm_parameter_path`)
//! stay in core and are called from the per-provider modules.

pub mod binance;
pub mod polymarket;

use std::{any::Any, fmt, sync::Arc};

use nautilus_common::cache::Cache;

use crate::{
    bolt_v3_adapters::{BoltV3ClientConfig, BoltV3ClientMappingError, BoltV3UpdownNowFn},
    bolt_v3_config::{BoltV3RootConfig, ClientBlock},
    bolt_v3_market_families::updown::{BoltV3MarketIdentityError, MarketIdentityPlan},
    bolt_v3_secrets::{BoltV3SecretError, ResolvedBoltV3Secrets},
};

pub trait ProviderResolvedSecrets: fmt::Debug + Send + Sync {
    fn venue_key(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
}

pub type ResolvedClientSecrets = Arc<dyn ProviderResolvedSecrets>;

pub trait SsmSecretResolver {
    fn resolve_secret(&mut self, region: &str, ssm_path: &str) -> Result<String, String>;
}

impl<F, E> SsmSecretResolver for F
where
    F: FnMut(&str, &str) -> Result<String, E>,
    E: fmt::Display,
{
    fn resolve_secret(&mut self, region: &str, ssm_path: &str) -> Result<String, String> {
        self(region, ssm_path).map_err(|error| error.to_string())
    }
}

pub struct ProviderSecretResolveContext<'a> {
    pub client_id_key: &'a str,
    pub region: &'a str,
    pub client_id: &'a ClientBlock,
}

pub struct ProviderAdapterMapContext<'a> {
    pub root: &'a BoltV3RootConfig,
    pub client_id_key: &'a str,
    pub client_id: &'a ClientBlock,
    pub resolved: &'a ResolvedBoltV3Secrets,
    pub plan: &'a MarketIdentityPlan,
    pub clock: BoltV3UpdownNowFn,
}

pub struct ProviderInstrumentReadinessContext<'a> {
    pub client_id_key: &'a str,
    pub venue_key: &'a str,
    pub plan: &'a MarketIdentityPlan,
    pub cache: &'a Cache,
    pub market_selection_timestamp_milliseconds: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderInstrumentReadinessStatus {
    Ready,
    Blocked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderInstrumentReadinessFact {
    pub client_id_key: String,
    pub strategy_instance_id: String,
    pub configured_target_id: String,
    pub status: ProviderInstrumentReadinessStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCredentialedBlock {
    Data,
    Execution,
}

impl ProviderCredentialedBlock {
    fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Execution => "execution",
        }
    }

    fn is_present(self, client_id: &ClientBlock) -> bool {
        match self {
            Self::Data => client_id.data.is_some(),
            Self::Execution => client_id.execution.is_some(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSecretRequirement {
    pub block: ProviderCredentialedBlock,
    pub consumer: &'static str,
}

pub struct ProviderBinding {
    pub key: &'static str,
    pub validate_client_id: fn(&str, &ClientBlock) -> Vec<String>,
    pub supported_market_families: &'static [&'static str],
    pub required_secret_blocks: &'static [ProviderSecretRequirement],
    pub credential_log_modules: &'static [&'static str],
    pub forbidden_env_vars: &'static [&'static str],
    pub resolve_secrets: for<'a> fn(
        ProviderSecretResolveContext<'a>,
        &mut dyn SsmSecretResolver,
    ) -> Result<ResolvedClientSecrets, BoltV3SecretError>,
    pub map_adapters: for<'a> fn(
        ProviderAdapterMapContext<'a>,
    ) -> Result<BoltV3ClientConfig, BoltV3ClientMappingError>,
    pub check_instrument_readiness: Option<
        for<'a> fn(
            ProviderInstrumentReadinessContext<'a>,
        )
            -> Result<Vec<ProviderInstrumentReadinessFact>, BoltV3MarketIdentityError>,
    >,
}

const PROVIDER_BINDINGS: &[ProviderBinding] = &[
    ProviderBinding {
        key: polymarket::KEY,
        validate_client_id: polymarket::validate_client_id,
        supported_market_families: polymarket::SUPPORTED_MARKET_FAMILIES,
        required_secret_blocks: polymarket::REQUIRED_SECRET_BLOCKS,
        credential_log_modules: polymarket::CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: polymarket::FORBIDDEN_ENV_VARS,
        resolve_secrets: polymarket::resolve_secrets,
        map_adapters: polymarket::map_adapters,
        check_instrument_readiness: Some(polymarket::check_instrument_readiness),
    },
    ProviderBinding {
        key: binance::KEY,
        validate_client_id: binance::validate_client_id,
        supported_market_families: binance::SUPPORTED_MARKET_FAMILIES,
        required_secret_blocks: binance::REQUIRED_SECRET_BLOCKS,
        credential_log_modules: binance::CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: binance::FORBIDDEN_ENV_VARS,
        resolve_secrets: binance::resolve_secrets,
        map_adapters: binance::map_adapters,
        check_instrument_readiness: None,
    },
];

pub fn provider_bindings() -> &'static [ProviderBinding] {
    PROVIDER_BINDINGS
}

pub fn binding_for_venue(key: &str) -> Option<&'static ProviderBinding> {
    provider_bindings()
        .iter()
        .find(|binding| binding.key == key)
}

/// Provider-owned NT adapter modules whose info logs can expose
/// credential metadata. The live-node builder consumes this provider
/// binding surface to install `WARN` module filters without hardcoding
/// concrete provider module paths in the live-node assembly layer.
pub fn credential_log_modules() -> impl Iterator<Item = &'static str> {
    provider_bindings()
        .iter()
        .flat_map(|binding| binding.credential_log_modules.iter().copied())
}

/// Family-agnostic surface read by core startup validation. Routes
/// each client block to its per-provider validator based on
/// venue key. Returns the full error list for the block.
pub fn validate_client_id_block(key: &str, client_id: &ClientBlock) -> Vec<String> {
    match binding_for_venue(client_id.venue.as_str()) {
        Some(binding) => {
            let mut errors = validate_required_secret_blocks(
                key,
                binding.key,
                client_id,
                binding.required_secret_blocks,
            );
            errors.extend((binding.validate_client_id)(key, client_id));
            errors
        }
        None => vec![format!(
            "clients.{key}.venue `{}` is not supported by this build",
            client_id.venue.as_str()
        )],
    }
}

fn validate_required_secret_blocks(
    key: &str,
    venue: &str,
    client_id: &ClientBlock,
    requirements: &[ProviderSecretRequirement],
) -> Vec<String> {
    let mut errors = Vec::new();
    if client_id.secrets.is_some() {
        return errors;
    }
    for requirement in requirements {
        if requirement.block.is_present(client_id) {
            errors.push(format!(
                "clients.{key} (venue={venue}) declares [{}] but is missing the required [secrets] block; \
                 the bolt-v3 secret contract requires SSM credential resolution for every {}",
                requirement.block.as_str(),
                requirement.consumer
            ));
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client_id_from_toml(text: &str) -> ClientBlock {
        toml::from_str(text).expect("test client should parse")
    }

    #[test]
    fn credential_log_modules_are_provider_owned() {
        let polymarket =
            binding_for_venue(polymarket::KEY).expect("Polymarket binding must be registered");
        assert_eq!(
            polymarket.credential_log_modules,
            polymarket::CREDENTIAL_LOG_MODULES
        );

        let binance = binding_for_venue(binance::KEY).expect("Binance binding must be registered");
        assert_eq!(
            binance.credential_log_modules,
            binance::CREDENTIAL_LOG_MODULES
        );
    }

    #[test]
    fn provider_required_secrets_rejects_credentialed_block_without_secrets() {
        let client_id = client_id_from_toml(
            r#"
            venue = "fake"

            [data]
            "#,
        );
        let requirement = ProviderSecretRequirement {
            block: ProviderCredentialedBlock::Data,
            consumer: "fake data adapter",
        };

        let errors =
            validate_required_secret_blocks("fake_instance", "fake", &client_id, &[requirement]);

        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("declares [data] but is missing the required [secrets] block"));
        assert!(errors[0].contains("fake data adapter"));
    }

    #[test]
    fn provider_required_secrets_ignores_absent_credentialed_block() {
        let client_id = client_id_from_toml(
            r#"
            venue = "fake"
            "#,
        );
        let requirement = ProviderSecretRequirement {
            block: ProviderCredentialedBlock::Execution,
            consumer: "fake execution adapter",
        };

        let errors =
            validate_required_secret_blocks("fake_instance", "fake", &client_id, &[requirement]);

        assert!(errors.is_empty());
    }
}
