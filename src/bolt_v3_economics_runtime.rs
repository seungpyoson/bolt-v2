use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock},
};

use rust_decimal::Decimal;

use crate::{
    bolt_v3_config::{
        EconomicsReportingConfig, EconomicsValuationLegConfig, EconomicsValuationOrientation,
        ExecutionEconomicsConfig, LoadedBoltV3Config,
    },
    bolt_v3_providers::{
        ProviderEconomicsAdapterBuildContext, ProviderEconomicsReplayAuthorityBuildContext,
        ProviderExecutionEconomicsBinding, binding_for_provider_key,
    },
    economics::{
        AccountId, AdmissionTreatment, CurrencyId, EconomicScope, EconomicsError,
        EconomicsInstrumentId, EconomicsQuote, EconomicsQuoteRequest, EdgeBasisEvidence,
        LiquidityRole, NativeUnitId, NetEdgeQuote, PlannedFillNotional, ProductSurfaceId,
        ReportingPolicyId, SnapshotId, SourceIdentity, SourceValidity, ValuationLeg,
        ValuationRoute, ValuationRouteId, VenueEconomicsAdapter, VenueEconomicsUnavailable,
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
    by_scope: Arc<RwLock<BTreeMap<AuthoritativeEconomicsKey, AuthoritativeVenueEconomicsInput>>>,
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
            by_scope: Arc::new(RwLock::new(by_scope)),
        })
    }

    /// Atomically replace one execution client's authority set.
    ///
    /// Runtime refreshers use this to publish rotating instruments without
    /// leaving retired scopes quotable. Other execution clients are untouched.
    pub fn replace_execution_client(
        &self,
        execution_client_id: &str,
        inputs: impl IntoIterator<Item = AuthoritativeVenueEconomicsInput>,
    ) -> Result<(), EconomicsRuntimeBindingError> {
        let mut replacement = BTreeMap::new();
        for input in inputs {
            if input.key.execution_client_id != execution_client_id {
                return Err(
                    EconomicsRuntimeBindingError::AuthoritativeExecutionClientMismatch {
                        expected_execution_client_id: execution_client_id.to_string(),
                        authoritative_execution_client_id: input.key.execution_client_id,
                    },
                );
            }
            let key = input.key.clone();
            if replacement.insert(key.clone(), input).is_some() {
                return Err(EconomicsRuntimeBindingError::DuplicateAuthoritativeInput {
                    execution_client_id: key.execution_client_id,
                    instrument_id: key.instrument_id,
                    product_surface_id: key.product_surface_id,
                });
            }
        }
        let mut current = self
            .by_scope
            .write()
            .map_err(|_| EconomicsRuntimeBindingError::AuthoritativeInputStoreUnavailable)?;
        current.retain(|key, _| key.execution_client_id != execution_client_id);
        current.extend(replacement);
        Ok(())
    }

    fn for_execution_client(
        &self,
        execution_client_id: &str,
    ) -> Result<Vec<AuthoritativeVenueEconomicsInput>, EconomicsRuntimeBindingError> {
        Ok(self
            .by_scope
            .read()
            .map_err(|_| EconomicsRuntimeBindingError::AuthoritativeInputStoreUnavailable)?
            .iter()
            .filter(|(key, _)| key.execution_client_id == execution_client_id)
            .map(|(_, input)| input.clone())
            .collect())
    }

    fn exact(
        &self,
        key: &AuthoritativeEconomicsKey,
    ) -> Result<Option<AuthoritativeVenueEconomicsInput>, EconomicsRuntimeBindingError> {
        Ok(self
            .by_scope
            .read()
            .map_err(|_| EconomicsRuntimeBindingError::AuthoritativeInputStoreUnavailable)?
            .get(key)
            .cloned())
    }
}

#[derive(Clone)]
struct BoundEconomicsScope {
    account_id: AccountId,
    adapter: Arc<dyn VenueEconomicsAdapter>,
    valuation_routes: Vec<ValuationRoute>,
}

#[derive(Clone)]
pub struct BoundExecutionEconomics {
    execution_client_id: String,
    provider_key: String,
    reporting_policy_id: ReportingPolicyId,
    reporting_currency: CurrencyId,
    config: ExecutionEconomicsConfig,
    execution: toml::Value,
    binding: ProviderExecutionEconomicsBinding,
    inputs: AuthoritativeEconomicsInputStore,
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

    pub(crate) fn planned_exit_horizon_ns(&self) -> Result<u64, EconomicsAdmissionError> {
        self.config
            .quote_validity_ms
            .checked_mul(NANOSECONDS_PER_MILLISECOND)
            .ok_or(EconomicsError::ArithmeticOverflow.into())
    }

    pub(crate) fn resting_order_refresh_margin_ns(&self) -> Result<u64, EconomicsAdmissionError> {
        self.config
            .resting_order_refresh_margin_ms
            .checked_mul(NANOSECONDS_PER_MILLISECOND)
            .ok_or(EconomicsError::ArithmeticOverflow.into())
    }

    pub(crate) fn cancel_retry_timeout_ns(&self) -> Result<u64, EconomicsAdmissionError> {
        self.config
            .cancel_retry_timeout_ms
            .get()
            .checked_mul(NANOSECONDS_PER_MILLISECOND)
            .ok_or(EconomicsError::ArithmeticOverflow.into())
    }

    fn build_scope(
        &self,
        input: &AuthoritativeVenueEconomicsInput,
    ) -> Result<BoundEconomicsScope, EconomicsRuntimeBindingError> {
        if input.provider_key != self.provider_key {
            return Err(
                EconomicsRuntimeBindingError::AuthoritativeProviderMismatch {
                    execution_client_id: self.execution_client_id.clone(),
                    configured_provider_key: self.provider_key.clone(),
                    authoritative_provider_key: input.provider_key.clone(),
                },
            );
        }
        let built = (self.binding.build_adapter)(ProviderEconomicsAdapterBuildContext {
            execution: &self.execution,
            config: &self.config,
            instrument_id: &input.key.instrument_id,
            product_surface_id: &input.key.product_surface_id,
            authority: input.authority.as_ref(),
        })
        .map_err(|message| {
            EconomicsRuntimeBindingError::AuthoritativeInputBuildFailed {
                execution_client_id: self.execution_client_id.clone(),
                instrument_id: input.key.instrument_id.clone(),
                product_surface_id: input.key.product_surface_id.clone(),
                message,
            }
        })?;
        if built.adapter.provider_key() != self.provider_key {
            return Err(
                EconomicsRuntimeBindingError::AuthoritativeInputBuildFailed {
                    execution_client_id: self.execution_client_id.clone(),
                    instrument_id: input.key.instrument_id.clone(),
                    product_surface_id: input.key.product_surface_id.clone(),
                    message: format!(
                        "provider builder returned `{}` instead of `{}`",
                        built.adapter.provider_key(),
                        self.provider_key
                    ),
                },
            );
        }
        let account_id = AccountId::try_new(built.account_id).map_err(|error| {
            EconomicsRuntimeBindingError::AuthoritativeInputBuildFailed {
                execution_client_id: self.execution_client_id.clone(),
                instrument_id: input.key.instrument_id.clone(),
                product_surface_id: input.key.product_surface_id.clone(),
                message: error.to_string(),
            }
        })?;
        let valuation_routes = build_valuation_routes(&self.config, &input.valuation_observations)
            .map_err(
                |message| EconomicsRuntimeBindingError::AuthoritativeValuationBuildFailed {
                    execution_client_id: self.execution_client_id.clone(),
                    instrument_id: input.key.instrument_id.clone(),
                    product_surface_id: input.key.product_surface_id.clone(),
                    message,
                },
            )?;
        Ok(BoundEconomicsScope {
            account_id,
            adapter: built.adapter,
            valuation_routes,
        })
    }

    fn scope_for_request(
        &self,
        request: &EconomicsQuoteRequest,
    ) -> Result<BoundEconomicsScope, EconomicsAdmissionError> {
        if request.execution_client_id.as_str() != self.execution_client_id {
            return Err(VenueEconomicsUnavailable::RequestScopeMismatch.into());
        }
        let key = AuthoritativeEconomicsKey {
            execution_client_id: self.execution_client_id.clone(),
            instrument_id: request.instrument_id.as_str().to_string(),
            product_surface_id: request.product_surface_id.as_str().to_string(),
        };
        let input = self
            .inputs
            .exact(&key)?
            .ok_or(VenueEconomicsUnavailable::MissingAuthoritativeSnapshot)?;
        self.build_scope(&input).map_err(Into::into)
    }

    pub(crate) fn request_authority(
        &self,
        instrument_id: &str,
    ) -> Result<BoundEconomicsRequestAuthority, EconomicsAdmissionError> {
        let instrument_id = EconomicsInstrumentId::try_new(instrument_id)?;
        let matching = self
            .inputs
            .for_execution_client(&self.execution_client_id)?;
        let mut matching = matching
            .iter()
            .filter(|input| input.key.instrument_id == instrument_id.as_str());
        let input = matching
            .next()
            .ok_or(VenueEconomicsUnavailable::MissingAuthoritativeSnapshot)?;
        if matching.next().is_some() {
            return Err(EconomicsAdmissionError::AmbiguousProductSurface);
        }
        let scope = self.build_scope(input)?;
        let product_surface_id = ProductSurfaceId::try_new(input.key.product_surface_id.clone())?;
        let edge_basis_policy_id = self
            .config
            .product_surface_policies
            .get(product_surface_id.as_str())
            .ok_or(EconomicsAdmissionError::EdgeBasisAuthorityMismatch)?;
        Ok(BoundEconomicsRequestAuthority {
            execution_client_id: self.execution_client_id.clone(),
            account_id: scope.account_id,
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
        let sizing = self.quote_sizing_inner(
            &intent.request,
            intent.policy,
            intent.gross_expected_value,
            intent.reservation_basis,
        )?;
        Ok(EconomicsAdmission {
            request: intent.request,
            order_binding: intent.order_binding,
            policy: intent.policy,
            quote: sizing.quote,
            net_edge: sizing.net_edge,
            reservation_basis: intent.reservation_basis,
            full_reservation_liability: sizing.full_reservation_liability,
        })
    }

    pub fn quote_sizing(
        &self,
        intent: EconomicsSizingIntent,
    ) -> Result<EconomicsSizingQuote, EconomicsAdmissionError> {
        self.quote_sizing_inner(
            &intent.request,
            intent.policy,
            intent.gross_expected_value,
            intent.reservation_basis,
        )
    }

    fn quote_sizing_inner(
        &self,
        request: &EconomicsQuoteRequest,
        policy: EconomicsAdmissionPolicy,
        gross_expected_value: Decimal,
        reservation_basis: Decimal,
    ) -> Result<EconomicsSizingQuote, EconomicsAdmissionError> {
        if reservation_basis <= Decimal::ZERO {
            return Err(EconomicsError::NonPositiveValue {
                field: "reservation_basis",
            }
            .into());
        }
        if request.reporting_policy_id != self.reporting_policy_id
            || request.reporting_currency != self.reporting_currency
        {
            return Err(EconomicsAdmissionError::ReportingAuthorityMismatch);
        }
        if self
            .config
            .product_surface_policies
            .get(request.product_surface_id.as_str())
            .is_none_or(|policy_id| policy_id != request.edge_basis_policy_id.as_str())
        {
            return Err(EconomicsAdmissionError::EdgeBasisAuthorityMismatch);
        }
        let scope = self.scope_for_request(request)?;
        if request.account_id != scope.account_id {
            return Err(VenueEconomicsUnavailable::RequestScopeMismatch.into());
        }
        let planned_fill_notional = PlannedFillNotional::from_legs(&request.planned_fill_legs)?;
        let edge_estimate = scope
            .adapter
            .resolve_edge_basis(request, planned_fill_notional)?;
        let configured_basis = self
            .config
            .edge_basis
            .get(request.edge_basis_policy_id.as_str())
            .ok_or(EconomicsAdmissionError::EdgeBasisAuthorityMismatch)?;
        if edge_estimate.source_snapshot_ids.is_empty()
            || edge_estimate.resolver_id.as_str() != configured_basis.resolver_id
            || edge_estimate.product_metadata_source.as_str()
                != configured_basis.product_metadata_source
            || edge_estimate.policy_version != configured_basis.policy_version
        {
            return Err(EconomicsAdmissionError::EdgeBasisAuthorityMismatch);
        }
        let estimate = scope.adapter.quote(request)?;
        let configured_valid_until_ns =
            configured_quote_deadline(&self.config, request, &estimate)?;
        let mut quote = validate_and_aggregate_quote(request, estimate, &scope.valuation_routes)?;
        quote.cap_valid_until_ns(configured_valid_until_ns.min(edge_estimate.valid_until_ns))?;
        let basis = EdgeBasisEvidence {
            policy_id: request.edge_basis_policy_id.clone(),
            resolver_id: edge_estimate.resolver_id,
            product_metadata_source: edge_estimate.product_metadata_source,
            policy_version: edge_estimate.policy_version,
            normalized_amount: edge_estimate.normalized_amount,
            scope: EconomicScope::Decision {
                decision_correlation_id: request.decision_correlation_id.clone(),
            },
            source_snapshot_ids: edge_estimate.source_snapshot_ids,
            valid_until_ns: edge_estimate.valid_until_ns.min(quote.valid_until_ns()),
        };
        let net_edge = fold_net_edge(gross_expected_value, &quote, basis)?;
        if let EconomicsAdmissionPolicy::TradingEdge {
            minimum_core_edge_ratio,
        } = policy
        {
            // Strategy policy may intentionally allow a negative raw threshold,
            // but shared economics never admits a non-positive fee-adjusted edge.
            let required_core_edge_ratio = minimum_core_edge_ratio.max(Decimal::ZERO);
            if net_edge.core_edge_ratio <= required_core_edge_ratio {
                return Err(EconomicsAdmissionError::CoreEdgeBelowMinimum {
                    minimum_core_edge_ratio: required_core_edge_ratio,
                    actual_core_edge_ratio: net_edge.core_edge_ratio,
                });
            }
        }
        let guaranteed_debit = if quote.core_total().is_sign_negative() {
            Decimal::ZERO
                .checked_sub(quote.core_total())
                .ok_or(EconomicsError::ArithmeticOverflow)?
        } else {
            Decimal::ZERO
        };
        let full_reservation_liability = reservation_basis
            .checked_add(guaranteed_debit)
            .ok_or(EconomicsError::ArithmeticOverflow)?;
        Ok(EconomicsSizingQuote {
            quote,
            net_edge,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EconomicsAdmissionPolicy {
    TradingEdge { minimum_core_edge_ratio: Decimal },
    RiskReduction,
}

impl EconomicsAdmissionPolicy {
    pub const fn purpose(self) -> EconomicsAdmissionPurpose {
        match self {
            Self::TradingEdge { .. } => EconomicsAdmissionPurpose::TradingEdge,
            Self::RiskReduction => EconomicsAdmissionPurpose::RiskReduction,
        }
    }
}

pub struct EconomicsAdmissionIntent {
    pub request: EconomicsQuoteRequest,
    pub order_binding: EconomicsOrderBinding,
    pub policy: EconomicsAdmissionPolicy,
    pub gross_expected_value: Decimal,
    pub reservation_basis: Decimal,
}

pub struct EconomicsSizingIntent {
    pub request: EconomicsQuoteRequest,
    pub policy: EconomicsAdmissionPolicy,
    pub gross_expected_value: Decimal,
    pub reservation_basis: Decimal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EconomicsSizingQuote {
    quote: EconomicsQuote,
    net_edge: NetEdgeQuote,
    full_reservation_liability: Decimal,
}

impl EconomicsSizingQuote {
    pub fn quote(&self) -> &EconomicsQuote {
        &self.quote
    }

    pub fn net_edge(&self) -> &NetEdgeQuote {
        &self.net_edge
    }

    pub const fn full_reservation_liability(&self) -> Decimal {
        self.full_reservation_liability
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EconomicsAdmission {
    request: EconomicsQuoteRequest,
    order_binding: EconomicsOrderBinding,
    policy: EconomicsAdmissionPolicy,
    quote: EconomicsQuote,
    net_edge: NetEdgeQuote,
    reservation_basis: Decimal,
    full_reservation_liability: Decimal,
}

impl EconomicsAdmission {
    pub fn request(&self) -> &EconomicsQuoteRequest {
        &self.request
    }

    pub const fn order_binding(&self) -> &EconomicsOrderBinding {
        &self.order_binding
    }

    pub const fn purpose(&self) -> EconomicsAdmissionPurpose {
        self.policy.purpose()
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

    #[cfg(test)]
    pub(crate) fn for_routing_test_with_validity(
        execution_client_id: &str,
        instrument_id: &str,
        order_side: crate::economics::OrderSide,
        order_binding: EconomicsOrderBinding,
        purpose: EconomicsAdmissionPurpose,
        reservation_basis: Decimal,
        full_reservation_liability: Decimal,
        valid_until_ns: u64,
    ) -> Self {
        use crate::economics::{
            DecisionCorrelationId, EdgeBasisAmount, EdgeBasisPolicyId, FormulaId, LifecyclePath,
            LiquidityRole, PlannedFillLeg, RoutingContext,
        };

        let edge_basis_policy_id =
            routing_test_id("routing-test-basis", EdgeBasisPolicyId::try_new);
        let decision_correlation_id =
            routing_test_id("routing-test-decision", DecisionCorrelationId::try_new);
        let request = EconomicsQuoteRequest {
            execution_client_id: routing_test_id(
                execution_client_id,
                crate::economics::ExecutionClientId::try_new,
            ),
            account_id: routing_test_id("routing-test-account", AccountId::try_new),
            instrument_id: routing_test_id(instrument_id, EconomicsInstrumentId::try_new),
            product_surface_id: routing_test_id("routing-test-surface", ProductSurfaceId::try_new),
            order_side,
            liquidity_role: LiquidityRole::Taker,
            planned_fill_legs: vec![PlannedFillLeg {
                price: Decimal::ONE,
                quantity: reservation_basis,
            }],
            routing: RoutingContext {
                attached_charge: None,
            },
            position: None,
            lifecycle_path: LifecyclePath::PlannedExit,
            reporting_policy_id: routing_test_id(
                "routing-test-reporting",
                ReportingPolicyId::try_new,
            ),
            reporting_currency: routing_test_id("USD", CurrencyId::try_new),
            edge_basis_policy_id: edge_basis_policy_id.clone(),
            requested_at_ns: 1,
            decision_correlation_id: decision_correlation_id.clone(),
        };
        let quote = validate_and_aggregate_quote(
            &request,
            VenueQuoteEstimate {
                authority: SourceValidity {
                    source: routing_test_id("routing-test-source", SourceIdentity::try_new),
                    snapshot_id: routing_test_id("routing-test-snapshot", SnapshotId::try_new),
                    source_at_ns: 1,
                    fetched_at_ns: 1,
                    valid_until_ns,
                },
                dependency_sources: Vec::new(),
                components: Vec::new(),
            },
            &[],
        )
        .expect("routing-test economics quote should validate");
        let basis = EdgeBasisEvidence {
            policy_id: edge_basis_policy_id,
            resolver_id: routing_test_id("routing-test-resolver", FormulaId::try_new),
            product_metadata_source: routing_test_id(
                "routing-test-product",
                SourceIdentity::try_new,
            ),
            policy_version: 1,
            normalized_amount: EdgeBasisAmount::try_new(reservation_basis)
                .expect("routing-test reservation should be positive"),
            scope: EconomicScope::Decision {
                decision_correlation_id,
            },
            source_snapshot_ids: vec![routing_test_id(
                "routing-test-product-snapshot",
                SnapshotId::try_new,
            )],
            valid_until_ns,
        };
        let net_edge =
            fold_net_edge(reservation_basis, &quote, basis).expect("routing-test edge should fold");
        Self {
            request,
            order_binding,
            policy: match purpose {
                EconomicsAdmissionPurpose::TradingEdge => EconomicsAdmissionPolicy::TradingEdge {
                    minimum_core_edge_ratio: Decimal::ZERO,
                },
                EconomicsAdmissionPurpose::RiskReduction => EconomicsAdmissionPolicy::RiskReduction,
            },
            quote,
            net_edge,
            reservation_basis,
            full_reservation_liability,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestingOrderEconomicsCancelReason {
    InvalidState,
    MakerGuaranteeLost,
    QuoteUnavailable,
    TermsChanged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestingOrderEconomicsRefresh {
    NotDue,
    Complete,
    Refreshed(Box<EconomicsAdmission>),
    CancelRequired(RestingOrderEconomicsCancelReason),
}

pub fn refresh_resting_order_economics(
    source: &BoundExecutionEconomics,
    prior: &EconomicsAdmission,
    remaining_quantity: Decimal,
    authorized_quantity_ceiling: Decimal,
    maker_guarantee_intact: bool,
    now_ns: u64,
) -> RestingOrderEconomicsRefresh {
    if remaining_quantity.is_sign_negative() || now_ns == 0 {
        return resting_cancel(RestingOrderEconomicsCancelReason::InvalidState);
    }
    if remaining_quantity.is_zero() {
        return RestingOrderEconomicsRefresh::Complete;
    }
    if prior.request.liquidity_role != LiquidityRole::GuaranteedMaker
        || prior.purpose() != EconomicsAdmissionPurpose::TradingEdge
    {
        return resting_cancel(RestingOrderEconomicsCancelReason::InvalidState);
    }
    if !maker_guarantee_intact {
        return resting_cancel(RestingOrderEconomicsCancelReason::MakerGuaranteeLost);
    }
    if now_ns > prior.quote.valid_until_ns() {
        return resting_cancel(RestingOrderEconomicsCancelReason::QuoteUnavailable);
    }
    let refresh_at_ns = source
        .resting_order_refresh_margin_ns()
        .ok()
        .and_then(|margin| prior.quote.valid_until_ns().checked_sub(margin));
    let Some(refresh_at_ns) = refresh_at_ns else {
        return resting_cancel(RestingOrderEconomicsCancelReason::InvalidState);
    };
    if now_ns < refresh_at_ns {
        return RestingOrderEconomicsRefresh::NotDue;
    }
    let [prior_leg] = prior.request.planned_fill_legs.as_slice() else {
        return resting_cancel(RestingOrderEconomicsCancelReason::InvalidState);
    };
    if authorized_quantity_ceiling <= Decimal::ZERO
        || remaining_quantity > authorized_quantity_ceiling
        || remaining_quantity > prior_leg.quantity
        || prior.reservation_basis <= Decimal::ZERO
    {
        return resting_cancel(RestingOrderEconomicsCancelReason::InvalidState);
    }
    let Some(quantity_ratio) = remaining_quantity.checked_div(prior_leg.quantity) else {
        return resting_cancel(RestingOrderEconomicsCancelReason::InvalidState);
    };
    let Some(reservation_basis) = prior.reservation_basis.checked_mul(quantity_ratio) else {
        return resting_cancel(RestingOrderEconomicsCancelReason::InvalidState);
    };
    let Some(gross_expected_value) = prior
        .net_edge
        .gross_expected_value
        .checked_mul(quantity_ratio)
    else {
        return resting_cancel(RestingOrderEconomicsCancelReason::InvalidState);
    };
    if reservation_basis <= Decimal::ZERO {
        return resting_cancel(RestingOrderEconomicsCancelReason::InvalidState);
    }
    let mut request = prior.request.clone();
    request.planned_fill_legs[0].quantity = remaining_quantity;
    request.requested_at_ns = now_ns;
    let refreshed = match source.quote_admission(EconomicsAdmissionIntent {
        request,
        order_binding: prior.order_binding.clone(),
        policy: prior.policy,
        gross_expected_value,
        reservation_basis,
    }) {
        Ok(admission) => admission,
        Err(_) => {
            return resting_cancel(RestingOrderEconomicsCancelReason::QuoteUnavailable);
        }
    };
    if !resting_economic_terms_match(prior, &refreshed) {
        return resting_cancel(RestingOrderEconomicsCancelReason::TermsChanged);
    }
    RestingOrderEconomicsRefresh::Refreshed(Box::new(refreshed))
}

fn resting_cancel(reason: RestingOrderEconomicsCancelReason) -> RestingOrderEconomicsRefresh {
    RestingOrderEconomicsRefresh::CancelRequired(reason)
}

fn resting_economic_terms_match(
    prior: &EconomicsAdmission,
    refreshed: &EconomicsAdmission,
) -> bool {
    fn ratio(amount: Decimal, basis: Decimal) -> Option<Decimal> {
        amount.checked_div(basis)
    }

    fn effect_terms_match(
        before: &crate::economics::SignedNativeEffect,
        after: &crate::economics::SignedNativeEffect,
        before_quantity: Decimal,
        after_quantity: Decimal,
    ) -> bool {
        use crate::economics::SignedNativeEffect;

        let same_shape = match (before, after) {
            (
                SignedNativeEffect::CurrencyAmount {
                    currency_id: before,
                    ..
                },
                SignedNativeEffect::CurrencyAmount {
                    currency_id: after, ..
                },
            ) => before == after,
            (
                SignedNativeEffect::AssetQuantity {
                    asset_id: before_asset,
                    inventory_application: before_application,
                    ..
                },
                SignedNativeEffect::AssetQuantity {
                    asset_id: after_asset,
                    inventory_application: after_application,
                    ..
                },
            ) => before_asset == after_asset && before_application == after_application,
            _ => false,
        };
        same_shape
            && ratio(before.amount(), before_quantity) == ratio(after.amount(), after_quantity)
    }

    fn optional_effect_terms_match(
        before: Option<&crate::economics::SignedNativeEffect>,
        after: Option<&crate::economics::SignedNativeEffect>,
        before_quantity: Decimal,
        after_quantity: Decimal,
    ) -> bool {
        match (before, after) {
            (Some(before), Some(after)) => {
                effect_terms_match(before, after, before_quantity, after_quantity)
            }
            (None, None) => true,
            _ => false,
        }
    }

    fn scope_terms_match(
        before: &crate::economics::EconomicScope,
        after: &crate::economics::EconomicScope,
    ) -> bool {
        use crate::economics::EconomicScope;

        match (before, after) {
            (
                EconomicScope::Decision {
                    decision_correlation_id: before,
                },
                EconomicScope::Decision {
                    decision_correlation_id: after,
                },
            ) => before == after,
            (
                EconomicScope::PositionInterval {
                    position_id: before_position,
                    starts_at_ns: before_start,
                    ends_at_ns: before_end,
                },
                EconomicScope::PositionInterval {
                    position_id: after_position,
                    starts_at_ns: after_start,
                    ends_at_ns: after_end,
                },
            ) => {
                before_position == after_position
                    && before_end.checked_sub(*before_start) == after_end.checked_sub(*after_start)
            }
            (
                EconomicScope::Action { action_id: before },
                EconomicScope::Action { action_id: after },
            ) => before == after,
            _ => false,
        }
    }

    let [prior_leg] = prior.request.planned_fill_legs.as_slice() else {
        return false;
    };
    let [refreshed_leg] = refreshed.request.planned_fill_legs.as_slice() else {
        return false;
    };
    let same_components = prior.quote.components().len() == refreshed.quote.components().len()
        && prior
            .quote
            .components()
            .iter()
            .zip(refreshed.quote.components())
            .all(|(before, after)| {
                let before = before.component();
                let after = after.component();
                let point_estimate_matches = match (&before.point_estimate, &after.point_estimate) {
                    (
                        crate::economics::PointEstimate::NonZero(before),
                        crate::economics::PointEstimate::NonZero(after),
                    ) => effect_terms_match(
                        before,
                        after,
                        prior_leg.quantity,
                        refreshed_leg.quantity,
                    ),
                    (
                        crate::economics::PointEstimate::ProvenZero { factor_id: before },
                        crate::economics::PointEstimate::ProvenZero { factor_id: after },
                    ) => before == after,
                    _ => false,
                };
                before.component_id == after.component_id
                    && before.class == after.class
                    && before.kind == after.kind
                    && scope_terms_match(&before.scope, &after.scope)
                    && before.admission_treatment == after.admission_treatment
                    && before.formula_id == after.formula_id
                    && before.source.source == after.source.source
                    && before.calculation_factors == after.calculation_factors
                    && point_estimate_matches
                    && optional_effect_terms_match(
                        before.debit_risk_bound.as_ref(),
                        after.debit_risk_bound.as_ref(),
                        prior_leg.quantity,
                        refreshed_leg.quantity,
                    )
            });
    let prior_basis = &prior.net_edge.basis;
    let refreshed_basis = &refreshed.net_edge.basis;
    let same_basis_authority = prior_basis.policy_id == refreshed_basis.policy_id
        && prior_basis.resolver_id == refreshed_basis.resolver_id
        && prior_basis.product_metadata_source == refreshed_basis.product_metadata_source
        && prior_basis.policy_version == refreshed_basis.policy_version
        && prior_basis.scope == refreshed_basis.scope;
    let same_scaled_totals = [
        (prior.quote.core_total(), refreshed.quote.core_total()),
        (
            prior.quote.forecast_total(),
            refreshed.quote.forecast_total(),
        ),
        (
            prior.net_edge.gross_expected_value,
            refreshed.net_edge.gross_expected_value,
        ),
        (
            prior.net_edge.core_net_edge,
            refreshed.net_edge.core_net_edge,
        ),
        (
            prior.net_edge.forecast_net_edge,
            refreshed.net_edge.forecast_net_edge,
        ),
        (
            prior.full_reservation_liability,
            refreshed.full_reservation_liability,
        ),
        (
            prior_basis.normalized_amount.amount(),
            refreshed_basis.normalized_amount.amount(),
        ),
    ]
    .into_iter()
    .all(|(before, after)| {
        ratio(before, prior.reservation_basis) == ratio(after, refreshed.reservation_basis)
    });
    same_components
        && prior.policy == refreshed.policy
        && same_basis_authority
        && same_scaled_totals
        && prior.quote.reporting_currency() == refreshed.quote.reporting_currency()
        && prior.quote.forecast_complete() == refreshed.quote.forecast_complete()
        && prior.quote.missing_forecast_component_ids()
            == refreshed.quote.missing_forecast_component_ids()
        && prior.net_edge.core_edge_ratio == refreshed.net_edge.core_edge_ratio
        && prior.net_edge.forecast_edge_ratio == refreshed.net_edge.forecast_edge_ratio
}

#[cfg(test)]
fn routing_test_id<T>(
    value: &str,
    constructor: impl FnOnce(String) -> Result<T, EconomicsError>,
) -> T {
    constructor(value.to_string()).expect("routing-test economics id should be valid")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EconomicsOrderBinding([u8; 32]);

impl EconomicsOrderBinding {
    pub const fn from_sha256(value: [u8; 32]) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EconomicsAdmissionError {
    Venue(VenueEconomicsUnavailable),
    Invalid(EconomicsError),
    AuthorityBinding(EconomicsRuntimeBindingError),
    EdgeBasisAuthorityMismatch,
    ReportingAuthorityMismatch,
    AmbiguousProductSurface,
    CoreEdgeBelowMinimum {
        minimum_core_edge_ratio: Decimal,
        actual_core_edge_ratio: Decimal,
    },
}

impl std::fmt::Display for EconomicsAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Venue(error) => error.fmt(f),
            Self::Invalid(error) => error.fmt(f),
            Self::AuthorityBinding(error) => error.fmt(f),
            Self::EdgeBasisAuthorityMismatch => {
                f.write_str("economics edge-basis authority does not match TOML")
            }
            Self::ReportingAuthorityMismatch => {
                f.write_str("economics reporting authority does not match root TOML")
            }
            Self::AmbiguousProductSurface => {
                f.write_str("economics instrument matches more than one product surface")
            }
            Self::CoreEdgeBelowMinimum {
                minimum_core_edge_ratio,
                actual_core_edge_ratio,
            } => write!(
                f,
                "economics core edge ratio {actual_core_edge_ratio} does not exceed required minimum {minimum_core_edge_ratio}"
            ),
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

impl From<EconomicsRuntimeBindingError> for EconomicsAdmissionError {
    fn from(value: EconomicsRuntimeBindingError) -> Self {
        Self::AuthorityBinding(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EconomicsRuntimeBindingError {
    AuthoritativeInputStoreUnavailable,
    AuthoritativeExecutionClientMismatch {
        expected_execution_client_id: String,
        authoritative_execution_client_id: String,
    },
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
            Self::AuthoritativeInputStoreUnavailable => {
                f.write_str("authoritative economics input store is unavailable")
            }
            Self::AuthoritativeExecutionClientMismatch {
                expected_execution_client_id,
                authoritative_execution_client_id,
            } => write!(
                f,
                "authoritative economics input belongs to execution client `{authoritative_execution_client_id}` instead of `{expected_execution_client_id}`"
            ),
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

/// Build one historical economics authority through the configured provider's
/// registry binding. Replay supplies captured provider bytes and neutral scope
/// identifiers; the substrate never selects or implements venue formulas.
pub fn authoritative_economics_input_from_replay(
    loaded: &LoadedBoltV3Config,
    execution_client_id: &str,
    instrument_id: &str,
    product_surface_id: &str,
    authority: &toml::Value,
) -> Result<AuthoritativeVenueEconomicsInput, EconomicsRuntimeBindingError> {
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
    let economics = binding.execution_economics.ok_or_else(|| {
        EconomicsRuntimeBindingError::ProviderWithoutEconomicsBinding {
            execution_client_id: execution_client_id.to_string(),
            provider_key: provider_key.to_string(),
        }
    })?;
    (economics.build_replay_authority)(ProviderEconomicsReplayAuthorityBuildContext {
        execution_client_id,
        instrument_id,
        product_surface_id,
        authority,
    })
    .map_err(
        |message| EconomicsRuntimeBindingError::AuthoritativeInputBuildFailed {
            execution_client_id: execution_client_id.to_string(),
            instrument_id: instrument_id.to_string(),
            product_surface_id: product_surface_id.to_string(),
            message,
        },
    )
}

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
    Ok(BoundExecutionEconomics {
        execution_client_id: execution_client_id.to_string(),
        provider_key: provider_key.to_string(),
        reporting_policy_id,
        reporting_currency,
        config,
        execution: execution.clone(),
        binding: economics_binding,
        inputs: inputs.clone(),
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
