use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use rust_decimal::Decimal;

use crate::{
    bolt_v3_config::{
        EconomicsReportingConfig, EconomicsValuationLegConfig, EconomicsValuationOrientation,
        ExecutionEconomicsConfig, LoadedBoltV3Config,
    },
    bolt_v3_providers::{ProviderEconomicsAdapterBuildContext, binding_for_provider_key},
    economics::{
        AccountId, AdmissionTreatment, CurrencyId, EconomicScope, EconomicsError,
        EconomicsInstrumentId, EconomicsQuote, EconomicsQuoteRequest, EdgeBasisEvidence,
        NativeUnitId, NetEdgeQuote, PlannedFillNotional, ProductSurfaceId, ReportingPolicyId,
        SnapshotId, SourceIdentity, SourceValidity, ValuationLeg, ValuationRoute, ValuationRouteId,
        VenueEconomicsAdapter, VenueEconomicsUnavailable, VenueEdgeBasisEstimate,
        VenueQuoteEstimate, fold_net_edge, validate_and_aggregate_quote,
    },
};

const NANOSECONDS_PER_MILLISECOND: u64 = 1_000_000;

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
    valuation_observations: Vec<AuthoritativeValuationObservation>,
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
            valuation_observations: Vec::new(),
        }
    }

    pub fn with_valuation_observations(
        mut self,
        observations: impl IntoIterator<Item = AuthoritativeValuationObservation>,
    ) -> Self {
        self.valuation_observations.extend(observations);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthoritativeValuationObservation {
    MarketQuote {
        client_id: String,
        instrument_id: String,
        base_currency: CurrencyId,
        quote_currency: CurrencyId,
        price: Decimal,
        snapshot_id: SnapshotId,
        observed_at_ns: u64,
        fetched_at_ns: u64,
        valid_until_ns: u64,
    },
    ProviderExactConversion {
        source_id: SourceIdentity,
        from_unit: CurrencyId,
        to_unit: CurrencyId,
        snapshot_id: SnapshotId,
        observed_at_ns: u64,
        fetched_at_ns: u64,
        valid_until_ns: u64,
    },
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
    account_id: AccountId,
    by_scope: BTreeMap<(String, String), BoundEconomicsScope>,
}

struct BoundEconomicsScope {
    adapter: Arc<dyn VenueEconomicsAdapter>,
    valuation_routes: Vec<ValuationRoute>,
}

impl ExecutionVenueEconomicsRouter {
    fn adapter_for_request(
        &self,
        request: &EconomicsQuoteRequest,
    ) -> Result<&BoundEconomicsScope, VenueEconomicsUnavailable> {
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
            .adapter
            .resolve_edge_basis(request, planned_fill_notional)
    }

    fn quote(
        &self,
        request: &EconomicsQuoteRequest,
    ) -> Result<VenueQuoteEstimate, VenueEconomicsUnavailable> {
        self.adapter_for_request(request)?.adapter.quote(request)
    }
}

#[derive(Clone)]
pub struct BoundExecutionEconomics {
    execution_client_id: String,
    provider_key: String,
    reporting_policy_id: ReportingPolicyId,
    reporting_currency: CurrencyId,
    config: ExecutionEconomicsConfig,
    adapter: Arc<ExecutionVenueEconomicsRouter>,
}

impl BoundExecutionEconomics {
    pub fn execution_client_id(&self) -> &str {
        &self.execution_client_id
    }

    pub fn provider_key(&self) -> &str {
        &self.provider_key
    }

    pub fn account_id(&self) -> &AccountId {
        &self.adapter.account_id
    }

    pub fn config(&self) -> &ExecutionEconomicsConfig {
        &self.config
    }

    pub fn adapter(&self) -> Arc<dyn VenueEconomicsAdapter> {
        self.adapter.clone()
    }

    pub(crate) fn request_authority(
        &self,
        instrument_id: &str,
    ) -> Result<BoundEconomicsRequestAuthority, EconomicsAdmissionError> {
        let instrument_id = EconomicsInstrumentId::try_new(instrument_id)?;
        let mut matching = self
            .adapter
            .by_scope
            .keys()
            .filter(|(configured_instrument, _)| configured_instrument == instrument_id.as_str());
        let (_, product_surface_id) = matching
            .next()
            .ok_or(VenueEconomicsUnavailable::MissingAuthoritativeSnapshot)?;
        if matching.next().is_some() {
            return Err(EconomicsAdmissionError::AmbiguousProductSurface);
        }
        let product_surface_id = ProductSurfaceId::try_new(product_surface_id.clone())?;
        let edge_basis_policy_id = self
            .config
            .product_surface_policies
            .get(product_surface_id.as_str())
            .ok_or(EconomicsAdmissionError::EdgeBasisAuthorityMismatch)?;
        Ok(BoundEconomicsRequestAuthority {
            execution_client_id: self.execution_client_id.clone(),
            account_id: self.account_id().clone(),
            product_surface_id: product_surface_id.clone(),
            reporting_policy_id: self.reporting_policy_id.clone(),
            reporting_currency: self.reporting_currency.clone(),
            edge_basis_policy_id: crate::economics::EdgeBasisPolicyId::try_new(
                edge_basis_policy_id.clone(),
            )?,
            carry_required: self
                .config
                .carry_surfaces
                .contains(product_surface_id.as_str()),
        })
    }

    pub fn quote_admission(
        &self,
        intent: EconomicsAdmissionIntent,
    ) -> Result<EconomicsAdmission, EconomicsAdmissionError> {
        if intent.reservation_basis <= Decimal::ZERO {
            return Err(EconomicsError::NonPositiveValue {
                field: "reservation_basis",
            }
            .into());
        }
        if intent.request.reporting_policy_id != self.reporting_policy_id
            || intent.request.reporting_currency != self.reporting_currency
        {
            return Err(EconomicsAdmissionError::ReportingAuthorityMismatch);
        }
        if self
            .config
            .product_surface_policies
            .get(intent.request.product_surface_id.as_str())
            .is_none_or(|policy_id| policy_id != intent.request.edge_basis_policy_id.as_str())
        {
            return Err(EconomicsAdmissionError::EdgeBasisAuthorityMismatch);
        }
        let scope = self.adapter.adapter_for_request(&intent.request)?;
        let planned_fill_notional =
            PlannedFillNotional::from_legs(&intent.request.planned_fill_legs)?;
        let edge_estimate = scope
            .adapter
            .resolve_edge_basis(&intent.request, planned_fill_notional)?;
        let configured_basis = self
            .config
            .edge_basis
            .get(intent.request.edge_basis_policy_id.as_str())
            .ok_or(EconomicsAdmissionError::EdgeBasisAuthorityMismatch)?;
        if edge_estimate.source_snapshot_ids.is_empty()
            || edge_estimate.resolver_id.as_str() != configured_basis.resolver_id
            || edge_estimate.product_metadata_source.as_str()
                != configured_basis.product_metadata_source
            || edge_estimate.policy_version != configured_basis.policy_version
        {
            return Err(EconomicsAdmissionError::EdgeBasisAuthorityMismatch);
        }
        let estimate = scope.adapter.quote(&intent.request)?;
        let configured_valid_until_ns =
            configured_quote_deadline(&self.config, &intent.request, &estimate)?;
        let mut quote =
            validate_and_aggregate_quote(&intent.request, estimate, &scope.valuation_routes)?;
        quote.cap_valid_until_ns(configured_valid_until_ns.min(edge_estimate.valid_until_ns))?;
        let basis = EdgeBasisEvidence {
            policy_id: intent.request.edge_basis_policy_id.clone(),
            resolver_id: edge_estimate.resolver_id,
            product_metadata_source: edge_estimate.product_metadata_source,
            policy_version: edge_estimate.policy_version,
            normalized_amount: edge_estimate.normalized_amount,
            scope: EconomicScope::Decision {
                decision_correlation_id: intent.request.decision_correlation_id.clone(),
            },
            source_snapshot_ids: edge_estimate.source_snapshot_ids,
            valid_until_ns: edge_estimate.valid_until_ns.min(quote.valid_until_ns()),
        };
        let net_edge = fold_net_edge(intent.gross_expected_value, &quote, basis)?;
        if intent.purpose == EconomicsAdmissionPurpose::TradingEdge
            && net_edge.core_net_edge <= Decimal::ZERO
        {
            return Err(EconomicsAdmissionError::NonPositiveNetEdge);
        }
        let guaranteed_debit = if quote.core_total().is_sign_negative() {
            Decimal::ZERO
                .checked_sub(quote.core_total())
                .ok_or(EconomicsError::ArithmeticOverflow)?
        } else {
            Decimal::ZERO
        };
        let full_reservation_liability = intent
            .reservation_basis
            .checked_add(guaranteed_debit)
            .ok_or(EconomicsError::ArithmeticOverflow)?;
        Ok(EconomicsAdmission {
            request: intent.request,
            purpose: intent.purpose,
            quote,
            net_edge,
            reservation_basis: intent.reservation_basis,
            full_reservation_liability,
        })
    }
}

pub(crate) struct BoundEconomicsRequestAuthority {
    pub execution_client_id: String,
    pub account_id: AccountId,
    pub product_surface_id: ProductSurfaceId,
    pub reporting_policy_id: ReportingPolicyId,
    pub reporting_currency: CurrencyId,
    pub edge_basis_policy_id: crate::economics::EdgeBasisPolicyId,
    pub carry_required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EconomicsAdmissionPurpose {
    TradingEdge,
    RiskReduction,
}

pub struct EconomicsAdmissionIntent {
    pub request: EconomicsQuoteRequest,
    pub purpose: EconomicsAdmissionPurpose,
    pub gross_expected_value: Decimal,
    pub reservation_basis: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EconomicsAdmission {
    request: EconomicsQuoteRequest,
    purpose: EconomicsAdmissionPurpose,
    quote: EconomicsQuote,
    net_edge: NetEdgeQuote,
    reservation_basis: Decimal,
    full_reservation_liability: Decimal,
}

impl EconomicsAdmission {
    pub fn request(&self) -> &EconomicsQuoteRequest {
        &self.request
    }

    pub const fn purpose(&self) -> EconomicsAdmissionPurpose {
        self.purpose
    }

    pub fn quote(&self) -> &EconomicsQuote {
        &self.quote
    }

    pub fn net_edge(&self) -> &NetEdgeQuote {
        &self.net_edge
    }

    pub const fn reservation_basis(&self) -> Decimal {
        self.reservation_basis
    }

    pub const fn full_reservation_liability(&self) -> Decimal {
        self.full_reservation_liability
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EconomicsAdmissionError {
    Venue(VenueEconomicsUnavailable),
    Invalid(EconomicsError),
    EdgeBasisAuthorityMismatch,
    ReportingAuthorityMismatch,
    AmbiguousProductSurface,
    NonPositiveNetEdge,
}

impl std::fmt::Display for EconomicsAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Venue(error) => error.fmt(f),
            Self::Invalid(error) => error.fmt(f),
            Self::EdgeBasisAuthorityMismatch => {
                f.write_str("economics edge-basis authority does not match TOML")
            }
            Self::ReportingAuthorityMismatch => {
                f.write_str("economics reporting authority does not match root TOML")
            }
            Self::AmbiguousProductSurface => {
                f.write_str("economics instrument matches more than one product surface")
            }
            Self::NonPositiveNetEdge => f.write_str("economics core net edge is not positive"),
        }
    }
}

impl std::error::Error for EconomicsAdmissionError {}

impl From<VenueEconomicsUnavailable> for EconomicsAdmissionError {
    fn from(value: VenueEconomicsUnavailable) -> Self {
        Self::Venue(value)
    }
}

impl From<EconomicsError> for EconomicsAdmissionError {
    fn from(value: EconomicsError) -> Self {
        Self::Invalid(value)
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
    AuthoritativeValuationBuildFailed {
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
            Self::AuthoritativeValuationBuildFailed {
                execution_client_id,
                instrument_id,
                product_surface_id,
                message,
            } => write!(
                f,
                "execution client `{execution_client_id}` could not bind valuation authority for `{instrument_id}` on `{product_surface_id}`: {message}"
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
    let reporting_policy_id =
        ReportingPolicyId::try_new(reporting.policy_id.clone()).map_err(|error| {
            EconomicsRuntimeBindingError::InvalidEconomicsConfig {
                execution_client_id: execution_client_id.to_string(),
                errors: vec![error.to_string()],
            }
        })?;
    let reporting_currency =
        CurrencyId::try_new(reporting.pnl_currency.clone()).map_err(|error| {
            EconomicsRuntimeBindingError::InvalidEconomicsConfig {
                execution_client_id: execution_client_id.to_string(),
                errors: vec![error.to_string()],
            }
        })?;
    let mut by_scope = BTreeMap::new();
    let mut bound_account_id = None;
    for input in resolved_inputs {
        let built = (economics_binding.build_adapter)(ProviderEconomicsAdapterBuildContext {
            execution,
            config: &config,
            instrument_id: &input.key.instrument_id,
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
        if built.adapter.provider_key() != provider_key {
            return Err(
                EconomicsRuntimeBindingError::AuthoritativeInputBuildFailed {
                    execution_client_id: execution_client_id.to_string(),
                    instrument_id: input.key.instrument_id.clone(),
                    product_surface_id: input.key.product_surface_id.clone(),
                    message: format!(
                        "provider builder returned `{}` instead of `{provider_key}`",
                        built.adapter.provider_key()
                    ),
                },
            );
        }
        let account_id = AccountId::try_new(built.account_id).map_err(|error| {
            EconomicsRuntimeBindingError::AuthoritativeInputBuildFailed {
                execution_client_id: execution_client_id.to_string(),
                instrument_id: input.key.instrument_id.clone(),
                product_surface_id: input.key.product_surface_id.clone(),
                message: error.to_string(),
            }
        })?;
        if bound_account_id
            .as_ref()
            .is_some_and(|bound| bound != &account_id)
        {
            return Err(
                EconomicsRuntimeBindingError::AuthoritativeInputBuildFailed {
                    execution_client_id: execution_client_id.to_string(),
                    instrument_id: input.key.instrument_id.clone(),
                    product_surface_id: input.key.product_surface_id.clone(),
                    message: "provider builder returned inconsistent accounts".to_string(),
                },
            );
        }
        bound_account_id = Some(account_id);
        let valuation_routes = build_valuation_routes(&config, &input.valuation_observations)
            .map_err(
                |message| EconomicsRuntimeBindingError::AuthoritativeValuationBuildFailed {
                    execution_client_id: execution_client_id.to_string(),
                    instrument_id: input.key.instrument_id.clone(),
                    product_surface_id: input.key.product_surface_id.clone(),
                    message,
                },
            )?;
        by_scope.insert(
            (
                input.key.instrument_id.clone(),
                input.key.product_surface_id.clone(),
            ),
            BoundEconomicsScope {
                adapter: built.adapter,
                valuation_routes,
            },
        );
    }
    let account_id = bound_account_id.ok_or_else(|| {
        EconomicsRuntimeBindingError::MissingAuthoritativeInput {
            execution_client_id: execution_client_id.to_string(),
        }
    })?;
    let adapter = Arc::new(ExecutionVenueEconomicsRouter {
        execution_client_id: execution_client_id.to_string(),
        provider_key: provider_key.to_string(),
        account_id,
        by_scope,
    });
    Ok(BoundExecutionEconomics {
        execution_client_id: execution_client_id.to_string(),
        provider_key: provider_key.to_string(),
        reporting_policy_id,
        reporting_currency,
        config,
        adapter,
    })
}

fn configured_quote_deadline(
    config: &ExecutionEconomicsConfig,
    request: &EconomicsQuoteRequest,
    estimate: &VenueQuoteEstimate,
) -> Result<u64, EconomicsAdmissionError> {
    let validity_ns = config
        .quote_validity_ms
        .checked_mul(NANOSECONDS_PER_MILLISECOND)
        .ok_or(EconomicsError::ArithmeticOverflow)?;
    let maximum_age_ns = config
        .quote_max_age_secs
        .checked_mul(1_000)
        .and_then(|milliseconds| milliseconds.checked_mul(NANOSECONDS_PER_MILLISECOND))
        .ok_or(EconomicsError::ArithmeticOverflow)?;
    let validity_deadline = request
        .requested_at_ns
        .checked_add(validity_ns)
        .ok_or(EconomicsError::ArithmeticOverflow)?;
    let mut source_deadline = configured_source_deadline(&estimate.authority, maximum_age_ns)?;
    for source in &estimate.dependency_sources {
        source_deadline = source_deadline.min(configured_source_deadline(source, maximum_age_ns)?);
    }
    for component in &estimate.components {
        if component.admission_treatment != AdmissionTreatment::ForecastOnly {
            source_deadline = source_deadline.min(configured_source_deadline(
                &component.source,
                maximum_age_ns,
            )?);
        }
    }
    Ok(validity_deadline.min(source_deadline))
}

fn configured_source_deadline(
    source: &SourceValidity,
    maximum_age_ns: u64,
) -> Result<u64, EconomicsAdmissionError> {
    Ok(source
        .source_at_ns
        .checked_add(maximum_age_ns)
        .ok_or(EconomicsError::ArithmeticOverflow)?
        .min(source.valid_until_ns))
}

fn build_valuation_routes(
    config: &ExecutionEconomicsConfig,
    observations: &[AuthoritativeValuationObservation],
) -> Result<Vec<ValuationRoute>, String> {
    let mut routes = Vec::with_capacity(config.valuation.routes.len());
    let mut route_scopes = BTreeSet::new();
    for (route_id, configured) in &config.valuation.routes {
        let mut legs = Vec::with_capacity(configured.legs.len());
        let mut route_valid_until_ns = u64::MAX;
        for configured_leg in &configured.legs {
            let leg = build_valuation_leg(config, configured_leg, observations)?;
            route_valid_until_ns = route_valid_until_ns.min(leg.valid_until_ns);
            legs.push(leg);
        }
        let route = ValuationRoute {
            route_id: ValuationRouteId::try_new(route_id.clone())
                .map_err(|error| error.to_string())?,
            from: NativeUnitId::Currency(
                CurrencyId::try_new(configured.from_unit.clone())
                    .map_err(|error| error.to_string())?,
            ),
            to: CurrencyId::try_new(configured.to_currency.clone())
                .map_err(|error| error.to_string())?,
            legs,
            valid_until_ns: route_valid_until_ns,
        };
        if !route_scopes.insert((route.from.clone(), route.to.clone())) {
            return Err(format!(
                "duplicate valuation route for `{}` to `{}`",
                route.from, route.to
            ));
        }
        routes.push(route);
    }
    Ok(routes)
}

fn build_valuation_leg(
    config: &ExecutionEconomicsConfig,
    configured: &EconomicsValuationLegConfig,
    observations: &[AuthoritativeValuationObservation],
) -> Result<ValuationLeg, String> {
    match configured {
        EconomicsValuationLegConfig::ProviderExactConversion {
            from_unit,
            to_unit,
            source_id,
            max_age_ms,
        } => {
            let expected_from =
                CurrencyId::try_new(from_unit.clone()).map_err(|error| error.to_string())?;
            let expected_to =
                CurrencyId::try_new(to_unit.clone()).map_err(|error| error.to_string())?;
            let expected_source =
                SourceIdentity::try_new(source_id.clone()).map_err(|error| error.to_string())?;
            let mut matching = observations
                .iter()
                .filter_map(|observation| match observation {
                    AuthoritativeValuationObservation::ProviderExactConversion {
                        source_id,
                        from_unit,
                        to_unit,
                        snapshot_id,
                        observed_at_ns,
                        fetched_at_ns,
                        valid_until_ns,
                    } if source_id == &expected_source
                        && from_unit == &expected_from
                        && to_unit == &expected_to =>
                    {
                        Some((
                            snapshot_id.clone(),
                            *observed_at_ns,
                            *fetched_at_ns,
                            *valid_until_ns,
                        ))
                    }
                    _ => None,
                });
            let (snapshot_id, observed_at_ns, fetched_at_ns, source_valid_until_ns) = matching
                .next()
                .ok_or_else(|| format!("missing exact valuation authority `{source_id}`"))?;
            if matching.next().is_some() {
                return Err(format!("duplicate exact valuation authority `{source_id}`"));
            }
            valuation_leg(
                NativeUnitId::Currency(expected_from),
                NativeUnitId::Currency(expected_to),
                Decimal::ONE,
                ResolvedValuationAuthority {
                    source: expected_source,
                    snapshot_id,
                    observed_at_ns,
                    fetched_at_ns,
                    valid_until_ns: source_valid_until_ns,
                },
                *max_age_ms,
            )
        }
        EconomicsValuationLegConfig::MarketQuote {
            from_unit,
            source_currency,
            to_unit,
            client_id,
            instrument_id,
            orientation,
            max_age_ms,
            ..
        } => {
            let identity_matches =
                config
                    .valuation
                    .exact_currency_identities
                    .values()
                    .any(|identity| {
                        identity.from_unit == *from_unit
                            && identity.source_currency == *source_currency
                    });
            if !identity_matches {
                return Err(format!(
                    "valuation unit `{from_unit}` has no exact identity as `{source_currency}`"
                ));
            }
            let expected_source_currency =
                CurrencyId::try_new(source_currency.clone()).map_err(|error| error.to_string())?;
            let expected_to =
                CurrencyId::try_new(to_unit.clone()).map_err(|error| error.to_string())?;
            let expected_instrument = EconomicsInstrumentId::try_new(instrument_id.clone())
                .map_err(|error| error.to_string())?;
            let valuation_source =
                SourceIdentity::try_new(client_id.clone()).map_err(|error| error.to_string())?;
            let mut matching = observations
                .iter()
                .filter_map(|observation| match observation {
                    AuthoritativeValuationObservation::MarketQuote {
                        client_id: observed_client,
                        instrument_id: observed_instrument,
                        base_currency,
                        quote_currency,
                        price,
                        snapshot_id,
                        observed_at_ns,
                        fetched_at_ns,
                        valid_until_ns,
                    } if observed_client == client_id
                        && observed_instrument == expected_instrument.as_str()
                        && match orientation {
                            EconomicsValuationOrientation::BaseToQuote => {
                                base_currency == &expected_source_currency
                                    && quote_currency == &expected_to
                            }
                            EconomicsValuationOrientation::QuoteToBase => {
                                quote_currency == &expected_source_currency
                                    && base_currency == &expected_to
                            }
                        } =>
                    {
                        Some((
                            *price,
                            snapshot_id.clone(),
                            *observed_at_ns,
                            *fetched_at_ns,
                            *valid_until_ns,
                        ))
                    }
                    _ => None,
                });
            let (price, snapshot_id, observed_at_ns, fetched_at_ns, source_valid_until_ns) =
                matching.next().ok_or_else(|| {
                    format!("missing market valuation authority `{client_id}`/`{instrument_id}`")
                })?;
            if matching.next().is_some() || price <= Decimal::ZERO {
                return Err(format!(
                    "ambiguous market valuation authority `{client_id}`/`{instrument_id}`"
                ));
            }
            let rate = match orientation {
                EconomicsValuationOrientation::BaseToQuote => price,
                EconomicsValuationOrientation::QuoteToBase => Decimal::ONE
                    .checked_div(price)
                    .ok_or_else(|| "market valuation inversion overflowed".to_string())?,
            };
            valuation_leg(
                NativeUnitId::Currency(
                    CurrencyId::try_new(from_unit.clone()).map_err(|error| error.to_string())?,
                ),
                NativeUnitId::Currency(expected_to),
                rate,
                ResolvedValuationAuthority {
                    source: valuation_source,
                    snapshot_id,
                    observed_at_ns,
                    fetched_at_ns,
                    valid_until_ns: source_valid_until_ns,
                },
                *max_age_ms,
            )
        }
    }
}

struct ResolvedValuationAuthority {
    source: SourceIdentity,
    snapshot_id: SnapshotId,
    observed_at_ns: u64,
    fetched_at_ns: u64,
    valid_until_ns: u64,
}

fn valuation_leg(
    from: NativeUnitId,
    to: NativeUnitId,
    to_units_per_from_unit: Decimal,
    authority: ResolvedValuationAuthority,
    max_age_ms: u64,
) -> Result<ValuationLeg, String> {
    if authority.observed_at_ns > authority.fetched_at_ns
        || authority.fetched_at_ns > authority.valid_until_ns
        || to_units_per_from_unit <= Decimal::ZERO
    {
        return Err("valuation authority timeline or rate is invalid".to_string());
    }
    let configured_valid_until_ns = authority
        .observed_at_ns
        .checked_add(
            max_age_ms
                .checked_mul(NANOSECONDS_PER_MILLISECOND)
                .ok_or_else(|| "valuation maximum age overflows nanoseconds".to_string())?,
        )
        .ok_or_else(|| "valuation validity deadline overflows".to_string())?;
    Ok(ValuationLeg {
        from,
        to,
        to_units_per_from_unit,
        source: authority.source,
        snapshot_id: authority.snapshot_id,
        observed_at_ns: authority.observed_at_ns,
        fetched_at_ns: authority.fetched_at_ns,
        valid_until_ns: configured_valid_until_ns.min(authority.valid_until_ns),
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
