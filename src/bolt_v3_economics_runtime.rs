use std::{any::Any, collections::BTreeMap, sync::Arc};

use crate::{
    bolt_v3_config::{EconomicsReportingConfig, ExecutionEconomicsConfig, LoadedBoltV3Config},
    bolt_v3_providers::{ProviderEconomicsAdapterBuildContext, binding_for_provider_key},
    economics::{
        EconomicsQuoteRequest, PlannedFillNotional, VenueEconomicsAdapter,
        VenueEconomicsUnavailable, VenueEdgeBasisEstimate, VenueQuoteEstimate,
    },
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct AuthoritativeEconomicsKey {
    execution_client_id: String,
    instrument_id: String,
    product_surface_id: String,
}

#[derive(Clone)]
pub struct AuthoritativeVenueEconomicsInput {
    key: AuthoritativeEconomicsKey,
    provider_key: String,
    authority: Arc<dyn Any + Send + Sync>,
}

impl AuthoritativeVenueEconomicsInput {
    pub(crate) fn from_provider_authority(
        execution_client_id: impl Into<String>,
        instrument_id: impl Into<String>,
        product_surface_id: impl Into<String>,
        provider_key: impl Into<String>,
        authority: Arc<dyn Any + Send + Sync>,
    ) -> Self {
        Self {
            key: AuthoritativeEconomicsKey {
                execution_client_id: execution_client_id.into(),
                instrument_id: instrument_id.into(),
                product_surface_id: product_surface_id.into(),
            },
            provider_key: provider_key.into(),
            authority,
        }
    }
}

#[derive(Clone, Default)]
pub struct AuthoritativeEconomicsInputStore {
    by_scope: Arc<BTreeMap<AuthoritativeEconomicsKey, AuthoritativeVenueEconomicsInput>>,
}

impl AuthoritativeEconomicsInputStore {
    pub fn try_new(
        inputs: impl IntoIterator<Item = AuthoritativeVenueEconomicsInput>,
    ) -> Result<Self, EconomicsRuntimeBindingError> {
        let mut by_scope = BTreeMap::new();
        for input in inputs {
            let key = input.key.clone();
            if by_scope.insert(key.clone(), input).is_some() {
                return Err(EconomicsRuntimeBindingError::DuplicateAuthoritativeInput {
                    execution_client_id: key.execution_client_id,
                    instrument_id: key.instrument_id,
                    product_surface_id: key.product_surface_id,
                });
            }
        }
        Ok(Self {
            by_scope: Arc::new(by_scope),
        })
    }

    fn for_execution_client(
        &self,
        execution_client_id: &str,
    ) -> impl Iterator<Item = &AuthoritativeVenueEconomicsInput> {
        self.by_scope
            .iter()
            .filter(move |(key, _)| key.execution_client_id == execution_client_id)
            .map(|(_, input)| input)
    }
}

struct ExecutionVenueEconomicsRouter {
    execution_client_id: String,
    provider_key: String,
    by_scope: BTreeMap<(String, String), Arc<dyn VenueEconomicsAdapter>>,
}

impl ExecutionVenueEconomicsRouter {
    fn adapter_for_request(
        &self,
        request: &EconomicsQuoteRequest,
    ) -> Result<&Arc<dyn VenueEconomicsAdapter>, VenueEconomicsUnavailable> {
        if request.execution_client_id.as_str() != self.execution_client_id {
            return Err(VenueEconomicsUnavailable::RequestScopeMismatch);
        }
        self.by_scope
            .get(&(
                request.instrument_id.as_str().to_string(),
                request.product_surface_id.as_str().to_string(),
            ))
            .ok_or(VenueEconomicsUnavailable::MissingAuthoritativeSnapshot)
    }
}

impl VenueEconomicsAdapter for ExecutionVenueEconomicsRouter {
    fn provider_key(&self) -> &str {
        &self.provider_key
    }

    fn resolve_edge_basis(
        &self,
        request: &EconomicsQuoteRequest,
        planned_fill_notional: PlannedFillNotional,
    ) -> Result<VenueEdgeBasisEstimate, VenueEconomicsUnavailable> {
        self.adapter_for_request(request)?
            .resolve_edge_basis(request, planned_fill_notional)
    }

    fn quote(
        &self,
        request: &EconomicsQuoteRequest,
    ) -> Result<VenueQuoteEstimate, VenueEconomicsUnavailable> {
        self.adapter_for_request(request)?.quote(request)
    }
}

#[derive(Clone)]
pub struct BoundExecutionEconomics {
    execution_client_id: String,
    provider_key: String,
    config: ExecutionEconomicsConfig,
    adapter: Arc<dyn VenueEconomicsAdapter>,
}

impl BoundExecutionEconomics {
    pub fn execution_client_id(&self) -> &str {
        &self.execution_client_id
    }

    pub fn provider_key(&self) -> &str {
        &self.provider_key
    }

    pub fn config(&self) -> &ExecutionEconomicsConfig {
        &self.config
    }

    pub fn adapter(&self) -> Arc<dyn VenueEconomicsAdapter> {
        self.adapter.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EconomicsRuntimeBindingError {
    MissingRootConfig,
    MissingExecutionClient {
        execution_client_id: String,
    },
    UnsupportedProvider {
        execution_client_id: String,
        provider_key: String,
    },
    ProviderWithoutEconomicsBinding {
        execution_client_id: String,
        provider_key: String,
    },
    MissingExecutionBlock {
        execution_client_id: String,
    },
    InvalidExecutionConfig {
        execution_client_id: String,
        message: String,
    },
    MissingEconomicsConfig {
        execution_client_id: String,
    },
    InvalidEconomicsConfig {
        execution_client_id: String,
        errors: Vec<String>,
    },
    MissingAuthoritativeInput {
        execution_client_id: String,
    },
    AuthoritativeProviderMismatch {
        execution_client_id: String,
        configured_provider_key: String,
        authoritative_provider_key: String,
    },
    AuthoritativeInputBuildFailed {
        execution_client_id: String,
        instrument_id: String,
        product_surface_id: String,
        message: String,
    },
    DuplicateAuthoritativeInput {
        execution_client_id: String,
        instrument_id: String,
        product_surface_id: String,
    },
}

impl std::fmt::Display for EconomicsRuntimeBindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingRootConfig => {
                f.write_str("root economics reporting configuration is required")
            }
            Self::MissingExecutionClient {
                execution_client_id,
            } => write!(
                f,
                "execution client `{execution_client_id}` is not configured"
            ),
            Self::UnsupportedProvider {
                execution_client_id,
                provider_key,
            } => write!(
                f,
                "execution client `{execution_client_id}` provider `{provider_key}` is not registered"
            ),
            Self::ProviderWithoutEconomicsBinding {
                execution_client_id,
                provider_key,
            } => write!(
                f,
                "execution client `{execution_client_id}` provider `{provider_key}` has no economics binding"
            ),
            Self::MissingExecutionBlock {
                execution_client_id,
            } => write!(
                f,
                "execution client `{execution_client_id}` has no execution configuration"
            ),
            Self::InvalidExecutionConfig {
                execution_client_id,
                message,
            } => write!(
                f,
                "execution client `{execution_client_id}` configuration is invalid: {message}"
            ),
            Self::MissingEconomicsConfig {
                execution_client_id,
            } => write!(
                f,
                "execution client `{execution_client_id}` has no economics configuration"
            ),
            Self::InvalidEconomicsConfig {
                execution_client_id,
                errors,
            } => write!(
                f,
                "execution client `{execution_client_id}` economics configuration is invalid: {}",
                errors.join("; ")
            ),
            Self::MissingAuthoritativeInput {
                execution_client_id,
            } => write!(
                f,
                "execution client `{execution_client_id}` has no authoritative economics input"
            ),
            Self::AuthoritativeProviderMismatch {
                execution_client_id,
                configured_provider_key,
                authoritative_provider_key,
            } => write!(
                f,
                "execution client `{execution_client_id}` configured provider `{configured_provider_key}` does not match authoritative provider `{authoritative_provider_key}`"
            ),
            Self::AuthoritativeInputBuildFailed {
                execution_client_id,
                instrument_id,
                product_surface_id,
                message,
            } => write!(
                f,
                "execution client `{execution_client_id}` could not build authoritative economics for `{instrument_id}` on `{product_surface_id}`: {message}"
            ),
            Self::DuplicateAuthoritativeInput {
                execution_client_id,
                instrument_id,
                product_surface_id,
            } => write!(
                f,
                "execution client `{execution_client_id}` has duplicate authoritative economics inputs for `{instrument_id}` on `{product_surface_id}`"
            ),
        }
    }
}

impl std::error::Error for EconomicsRuntimeBindingError {}

pub fn bind_execution_economics(
    loaded: &LoadedBoltV3Config,
    execution_client_id: &str,
    inputs: &AuthoritativeEconomicsInputStore,
) -> Result<BoundExecutionEconomics, EconomicsRuntimeBindingError> {
    let reporting = loaded
        .root
        .economics
        .as_ref()
        .map(|economics| &economics.reporting)
        .ok_or(EconomicsRuntimeBindingError::MissingRootConfig)?;
    let client = loaded
        .root
        .clients
        .get(execution_client_id)
        .ok_or_else(|| EconomicsRuntimeBindingError::MissingExecutionClient {
            execution_client_id: execution_client_id.to_string(),
        })?;
    let provider_key = client.venue.as_str();
    let binding = binding_for_provider_key(provider_key).ok_or_else(|| {
        EconomicsRuntimeBindingError::UnsupportedProvider {
            execution_client_id: execution_client_id.to_string(),
            provider_key: provider_key.to_string(),
        }
    })?;
    let resolved_inputs = inputs
        .for_execution_client(execution_client_id)
        .collect::<Vec<_>>();
    if resolved_inputs.is_empty() {
        return Err(EconomicsRuntimeBindingError::MissingAuthoritativeInput {
            execution_client_id: execution_client_id.to_string(),
        });
    }
    for input in &resolved_inputs {
        if input.provider_key != provider_key {
            return Err(
                EconomicsRuntimeBindingError::AuthoritativeProviderMismatch {
                    execution_client_id: execution_client_id.to_string(),
                    configured_provider_key: provider_key.to_string(),
                    authoritative_provider_key: input.provider_key.clone(),
                },
            );
        }
    }
    let economics_binding = binding.execution_economics.ok_or_else(|| {
        EconomicsRuntimeBindingError::ProviderWithoutEconomicsBinding {
            execution_client_id: execution_client_id.to_string(),
            provider_key: provider_key.to_string(),
        }
    })?;
    let execution = client.execution.as_ref().ok_or_else(|| {
        EconomicsRuntimeBindingError::MissingExecutionBlock {
            execution_client_id: execution_client_id.to_string(),
        }
    })?;
    let config = (economics_binding.load_config)(execution)
        .map_err(
            |message| EconomicsRuntimeBindingError::InvalidExecutionConfig {
                execution_client_id: execution_client_id.to_string(),
                message,
            },
        )?
        .ok_or_else(|| EconomicsRuntimeBindingError::MissingEconomicsConfig {
            execution_client_id: execution_client_id.to_string(),
        })?;
    validate_economics_config(execution_client_id, &config, reporting)?;
    let mut by_scope = BTreeMap::new();
    for input in resolved_inputs {
        let adapter = (economics_binding.build_adapter)(ProviderEconomicsAdapterBuildContext {
            config: &config,
            product_surface_id: &input.key.product_surface_id,
            authority: input.authority.as_ref(),
        })
        .map_err(|message| {
            EconomicsRuntimeBindingError::AuthoritativeInputBuildFailed {
                execution_client_id: execution_client_id.to_string(),
                instrument_id: input.key.instrument_id.clone(),
                product_surface_id: input.key.product_surface_id.clone(),
                message,
            }
        })?;
        by_scope.insert(
            (
                input.key.instrument_id.clone(),
                input.key.product_surface_id.clone(),
            ),
            adapter,
        );
    }
    let adapter: Arc<dyn VenueEconomicsAdapter> = Arc::new(ExecutionVenueEconomicsRouter {
        execution_client_id: execution_client_id.to_string(),
        provider_key: provider_key.to_string(),
        by_scope,
    });
    Ok(BoundExecutionEconomics {
        execution_client_id: execution_client_id.to_string(),
        provider_key: provider_key.to_string(),
        config,
        adapter,
    })
}

fn validate_economics_config(
    execution_client_id: &str,
    config: &ExecutionEconomicsConfig,
    reporting: &EconomicsReportingConfig,
) -> Result<(), EconomicsRuntimeBindingError> {
    let errors = config.validate_common(reporting);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(EconomicsRuntimeBindingError::InvalidEconomicsConfig {
            execution_client_id: execution_client_id.to_string(),
            errors,
        })
    }
}
