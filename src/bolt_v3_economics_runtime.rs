use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
    time::Duration,
};

use async_trait::async_trait;
use nautilus_model::{
    identifiers::{InstrumentId, Venue},
    instruments::InstrumentAny,
};
use rust_decimal::Decimal;

use crate::economics::{
    EconomicQuote, EconomicQuoteRequest, EconomicsUnavailable, EdgeBasisEvidence, NativeUnitId,
    NetEdgeQuote, SignedNativeEffect, SnapshotId, ValuationProvider, ValuationRequest,
    ValuationRoute, ValuationRouteId, VenueEconomicsAdapter, fold_net_edge,
    validate_and_aggregate_quote, value_with_route,
};

use crate::bolt_v3_economics_config::{ValuationConfig, ValuationLegConfig, ValuationOrientation};

pub struct EconomicsAdmissionIntent {
    pub request: EconomicQuoteRequest,
    pub order_binding: EconomicsOrderBinding,
    pub purpose: EconomicsAdmissionPurpose,
    pub gross_expected_value: Decimal,
    pub edge_basis: EdgeBasisEvidence,
    pub valuation_provider: Arc<dyn ValuationProvider>,
    pub base_reservation_notional: Decimal,
}

pub struct EconomicsAdmissionQuoteIntent {
    pub request: EconomicQuoteRequest,
    pub order_binding: EconomicsOrderBinding,
    pub purpose: EconomicsAdmissionPurpose,
    pub gross_expected_value: Decimal,
    pub base_reservation_notional: Decimal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EconomicsAdmissionPurpose {
    TradingEdge,
    RiskReduction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EconomicsOrderBinding {
    sha256: sha2::digest::Output<sha2::Sha256>,
}

impl EconomicsOrderBinding {
    pub fn from_sha256(sha256: sha2::digest::Output<sha2::Sha256>) -> Self {
        Self { sha256 }
    }

    pub fn sha256(&self) -> &sha2::digest::Output<sha2::Sha256> {
        &self.sha256
    }
}

pub trait EconomicsAdmissionSource: Send + Sync {
    fn resolve_product_surface(
        &self,
        execution_client_id: &crate::economics::ExecutionClientId,
        instrument_id: &crate::economics::InstrumentId,
        candidates: &[crate::economics::ProductSurfaceId],
    ) -> Result<crate::economics::ProductSurfaceId, EconomicsUnavailable>;

    fn quote_admission(
        &self,
        intent: EconomicsAdmissionQuoteIntent,
    ) -> Result<EconomicsAdmission, EconomicsUnavailable>;
}

#[derive(Clone)]
pub struct AuthoritativeEconomicsQuoteDependencies {
    pub provider_key: String,
    pub refreshed_at_ns: u64,
    pub adapter: Arc<dyn VenueEconomicsAdapter>,
    pub edge_basis: AuthoritativeEdgeBasis,
    pub valuation_provider: Arc<dyn ValuationProvider>,
}

pub struct ProviderEconomicsAuthoritySnapshot {
    pub refreshed_at_ns: u64,
    pub product_surface_id: String,
    pub adapter: Arc<dyn VenueEconomicsAdapter>,
    pub edge_basis: AuthoritativeEdgeBasis,
    pub valuation_observations: Vec<AuthoritativeValuationObservation>,
}

pub struct ProviderEconomicsAuthorityRefresh {
    pub instrument_id: InstrumentId,
    pub snapshot: anyhow::Result<ProviderEconomicsAuthoritySnapshot>,
}

pub trait EconomicsReceiptClock: Send + Sync {
    fn now_ns(&self) -> anyhow::Result<u64>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EconomicsSourceReceipt {
    pub fetched_at_ns: u64,
    pub valid_until_ns: u64,
}

pub fn capture_economics_source_receipt(
    receipt_clock: &dyn EconomicsReceiptClock,
    max_age_ns: u64,
) -> anyhow::Result<EconomicsSourceReceipt> {
    let fetched_at_ns = receipt_clock.now_ns()?;
    let valid_until_ns = fetched_at_ns
        .checked_add(max_age_ns)
        .ok_or(EconomicsUnavailable::InvalidQuoteValidityPolicy)?;
    Ok(EconomicsSourceReceipt {
        fetched_at_ns,
        valid_until_ns,
    })
}

#[cfg(test)]
mod source_receipt_tests {
    use super::*;

    #[test]
    fn delayed_response_is_available_only_at_its_receipt_time() {
        let request_started_at_ns = 100;
        let response_received_at_ns = 250;
        let receipt = capture_economics_source_receipt(&|| Ok(response_received_at_ns), 25)
            .expect("receipt timeline should build");

        assert!(receipt.fetched_at_ns > request_started_at_ns);
        assert_eq!(receipt.fetched_at_ns, response_received_at_ns);
        assert_eq!(receipt.valid_until_ns, 275);
    }

    #[test]
    fn receipt_validity_overflow_fails_closed() {
        assert!(capture_economics_source_receipt(&|| Ok(u64::MAX), 1).is_err());
    }
}

impl<F> EconomicsReceiptClock for F
where
    F: Fn() -> anyhow::Result<u64> + Send + Sync,
{
    fn now_ns(&self) -> anyhow::Result<u64> {
        self()
    }
}

#[async_trait(?Send)]
pub trait ProviderEconomicsAuthority: Send + Sync {
    fn execution_client_id(&self) -> &str;
    fn provider_key(&self) -> &str;
    fn venue(&self) -> Venue;
    fn economics_config(&self) -> &crate::bolt_v3_economics_config::ExecutionEconomicsConfig;

    async fn refresh_batch(
        &self,
        instruments: Vec<InstrumentAny>,
        receipt_clock: &dyn EconomicsReceiptClock,
    ) -> anyhow::Result<Vec<ProviderEconomicsAuthorityRefresh>>;
}

pub struct ConfiguredValuationProvider {
    routes: BTreeMap<(NativeUnitId, NativeUnitId), ValuationRoute>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthoritativeValuationObservation {
    MarketQuote {
        client_id: String,
        instrument_id: String,
        price: Decimal,
        snapshot_id: SnapshotId,
        observed_at_ns: u64,
        fetched_at_ns: u64,
        valid_until_ns: u64,
    },
    ProviderConversion {
        source_id: String,
        from_unit: NativeUnitId,
        to_unit: NativeUnitId,
        rate: Decimal,
        snapshot_id: SnapshotId,
        observed_at_ns: u64,
        fetched_at_ns: u64,
        valid_until_ns: u64,
    },
}

impl AuthoritativeValuationObservation {
    pub const fn fetched_at_ns(&self) -> u64 {
        match self {
            Self::MarketQuote { fetched_at_ns, .. }
            | Self::ProviderConversion { fetched_at_ns, .. } => *fetched_at_ns,
        }
    }
}

impl ConfiguredValuationProvider {
    pub fn from_routes(routes: Vec<ValuationRoute>) -> Result<Self, EconomicsUnavailable> {
        let mut indexed = BTreeMap::new();
        for route in routes {
            let key = (route.from_unit.clone(), route.to_currency.clone());
            if indexed.insert(key, route).is_some() {
                return Err(EconomicsUnavailable::AmbiguousQuoteAuthority);
            }
        }
        Ok(Self { routes: indexed })
    }

    pub fn from_config(
        config: &ValuationConfig,
        observations: &[AuthoritativeValuationObservation],
    ) -> Result<Self, EconomicsUnavailable> {
        let mut routes = Vec::with_capacity(config.routes.len());
        for (route_id, configured) in &config.routes {
            let mut legs = Vec::with_capacity(configured.legs.len());
            let mut route_valid_until_ns = u64::MAX;
            for configured_leg in &configured.legs {
                let (
                    from_unit,
                    to_unit,
                    rate,
                    snapshot_id,
                    observed_at_ns,
                    fetched_at_ns,
                    source_valid_until_ns,
                    max_age_ms,
                ) = resolve_valuation_leg(configured_leg, observations)?;
                let max_age_ns = u64::try_from(Duration::from_millis(max_age_ms).as_nanos())
                    .map_err(|_| EconomicsUnavailable::InvalidQuoteValidityPolicy)?;
                let configured_valid_until_ns = observed_at_ns
                    .checked_add(max_age_ns)
                    .ok_or(EconomicsUnavailable::InvalidQuoteValidityPolicy)?;
                let valid_until_ns = configured_valid_until_ns.min(source_valid_until_ns);
                route_valid_until_ns = route_valid_until_ns.min(valid_until_ns);
                legs.push(crate::economics::ValuationLegEvidence {
                    from_unit,
                    to_unit,
                    rate,
                    source_snapshot_id: snapshot_id,
                    observed_at_ns,
                    fetched_at_ns,
                    valid_until_ns,
                });
            }
            routes.push(ValuationRoute {
                route_id: ValuationRouteId::new(route_id.clone())?,
                from_unit: NativeUnitId::new(configured.from_unit.clone())?,
                to_currency: NativeUnitId::new(configured.to_currency.clone())?,
                legs,
                valid_until_ns: route_valid_until_ns,
            });
        }
        Self::from_routes(routes)
    }
}

type ResolvedValuationLeg = (
    NativeUnitId,
    NativeUnitId,
    Decimal,
    SnapshotId,
    u64,
    u64,
    u64,
    u64,
);

fn resolve_valuation_leg(
    configured: &ValuationLegConfig,
    observations: &[AuthoritativeValuationObservation],
) -> Result<ResolvedValuationLeg, EconomicsUnavailable> {
    let (
        from_unit,
        to_unit,
        rate,
        snapshot_id,
        observed_at_ns,
        fetched_at_ns,
        valid_until_ns,
        max_age_ms,
    ) = match configured {
        ValuationLegConfig::MarketQuote {
            from_unit,
            to_unit,
            client_id,
            instrument_id,
            orientation,
            max_age_ms,
            ..
        } => {
            let mut matching = observations
                .iter()
                .filter_map(|observation| match observation {
                    AuthoritativeValuationObservation::MarketQuote {
                        client_id: observed_client,
                        instrument_id: observed_instrument,
                        price,
                        snapshot_id,
                        observed_at_ns,
                        fetched_at_ns,
                        valid_until_ns,
                    } if observed_client == client_id && observed_instrument == instrument_id => {
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
            let (price, snapshot_id, observed_at_ns, fetched_at_ns, valid_until_ns) = matching
                .next()
                .ok_or(EconomicsUnavailable::MissingQuoteAuthority)?;
            if matching.next().is_some() || price <= Decimal::ZERO {
                return Err(EconomicsUnavailable::AmbiguousQuoteAuthority);
            }
            let rate = match orientation {
                ValuationOrientation::BaseToQuote => price,
                ValuationOrientation::QuoteToBase => Decimal::ONE
                    .checked_div(price)
                    .ok_or(EconomicsUnavailable::InvalidDecimal)?,
            };
            (
                NativeUnitId::new(from_unit.clone())?,
                NativeUnitId::new(to_unit.clone())?,
                rate,
                snapshot_id,
                observed_at_ns,
                fetched_at_ns,
                valid_until_ns,
                *max_age_ms,
            )
        }
        ValuationLegConfig::ProviderConversion {
            from_unit,
            to_unit,
            source_id,
            max_age_ms,
        } => {
            let expected_from = NativeUnitId::new(from_unit.clone())?;
            let expected_to = NativeUnitId::new(to_unit.clone())?;
            let mut matching = observations
                .iter()
                .filter_map(|observation| match observation {
                    AuthoritativeValuationObservation::ProviderConversion {
                        source_id: observed_source,
                        from_unit,
                        to_unit,
                        rate,
                        snapshot_id,
                        observed_at_ns,
                        fetched_at_ns,
                        valid_until_ns,
                    } if observed_source == source_id
                        && from_unit == &expected_from
                        && to_unit == &expected_to =>
                    {
                        Some((
                            *rate,
                            snapshot_id.clone(),
                            *observed_at_ns,
                            *fetched_at_ns,
                            *valid_until_ns,
                        ))
                    }
                    _ => None,
                });
            let (rate, snapshot_id, observed_at_ns, fetched_at_ns, valid_until_ns) = matching
                .next()
                .ok_or(EconomicsUnavailable::MissingQuoteAuthority)?;
            if matching.next().is_some() || rate <= Decimal::ZERO {
                return Err(EconomicsUnavailable::AmbiguousQuoteAuthority);
            }
            (
                expected_from,
                expected_to,
                rate,
                snapshot_id,
                observed_at_ns,
                fetched_at_ns,
                valid_until_ns,
                *max_age_ms,
            )
        }
    };
    if observed_at_ns > fetched_at_ns || fetched_at_ns > valid_until_ns {
        return Err(EconomicsUnavailable::InvalidQuoteValidityPolicy);
    }
    Ok((
        from_unit,
        to_unit,
        rate,
        snapshot_id,
        observed_at_ns,
        fetched_at_ns,
        valid_until_ns,
        max_age_ms,
    ))
}

impl ValuationProvider for ConfiguredValuationProvider {
    fn value(
        &self,
        effect: &SignedNativeEffect,
        request: &ValuationRequest,
    ) -> Result<crate::economics::ValuationEvidence, EconomicsUnavailable> {
        let route = self
            .routes
            .get(&(effect.unit().clone(), request.reporting_unit.clone()));
        value_with_route(
            effect,
            &request.reporting_unit,
            route,
            request.requested_at_ns,
        )
    }
}

pub fn identity_valuation_provider() -> Arc<dyn ValuationProvider> {
    Arc::new(ConfiguredValuationProvider {
        routes: BTreeMap::new(),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthoritativeEdgeBasis {
    pub resolver_id: crate::economics::FormulaId,
    pub product_metadata_source: crate::economics::SourceId,
    pub policy_version: u64,
    pub source_snapshot_ids: Vec<SnapshotId>,
    pub valid_until_ns: u64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct AuthoritativeEconomicsKey {
    execution_client_id: String,
    instrument_id: String,
    product_surface_id: String,
}

#[derive(Clone, Default)]
pub struct AuthoritativeEconomicsInputStore {
    entries:
        Arc<RwLock<BTreeMap<AuthoritativeEconomicsKey, AuthoritativeEconomicsQuoteDependencies>>>,
}

impl AuthoritativeEconomicsInputStore {
    pub fn publish(
        &self,
        execution_client_id: &str,
        instrument_id: &str,
        product_surface_id: &str,
        dependencies: AuthoritativeEconomicsQuoteDependencies,
    ) -> Result<(), EconomicsUnavailable> {
        let key = AuthoritativeEconomicsKey {
            execution_client_id: crate::economics::ExecutionClientId::new(execution_client_id)?
                .as_str()
                .to_string(),
            instrument_id: crate::economics::InstrumentId::new(instrument_id)?
                .as_str()
                .to_string(),
            product_surface_id: crate::economics::ProductSurfaceId::new(product_surface_id)?
                .as_str()
                .to_string(),
        };
        self.entries
            .write()
            .map_err(|_| EconomicsUnavailable::AmbiguousQuoteAuthority)?
            .insert(key, dependencies);
        Ok(())
    }

    fn dependencies(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<AuthoritativeEconomicsQuoteDependencies, EconomicsUnavailable> {
        let key = AuthoritativeEconomicsKey {
            execution_client_id: request.execution_client_id.as_str().to_string(),
            instrument_id: request.instrument_id.as_str().to_string(),
            product_surface_id: request.product_surface_id.as_str().to_string(),
        };
        self.entries
            .read()
            .map_err(|_| EconomicsUnavailable::AmbiguousQuoteAuthority)?
            .get(&key)
            .cloned()
            .ok_or(EconomicsUnavailable::MissingQuoteAuthority)
    }

    fn resolve_product_surface(
        &self,
        execution_client_id: &crate::economics::ExecutionClientId,
        instrument_id: &crate::economics::InstrumentId,
        provider_key: &str,
        candidates: &[crate::economics::ProductSurfaceId],
    ) -> Result<crate::economics::ProductSurfaceId, EconomicsUnavailable> {
        let entries = self
            .entries
            .read()
            .map_err(|_| EconomicsUnavailable::AmbiguousQuoteAuthority)?;
        let mut matches = entries
            .iter()
            .filter(|(key, dependencies)| {
                key.execution_client_id == execution_client_id.as_str()
                    && key.instrument_id == instrument_id.as_str()
                    && dependencies.provider_key == provider_key
                    && candidates
                        .iter()
                        .any(|candidate| candidate.as_str() == key.product_surface_id)
            })
            .map(|(key, _)| {
                crate::economics::ProductSurfaceId::new(key.product_surface_id.clone())
            });
        let selected = matches
            .next()
            .ok_or(EconomicsUnavailable::MissingQuoteAuthority)??;
        if matches.next().is_some() {
            return Err(EconomicsUnavailable::AmbiguousQuoteAuthority);
        }
        Ok(selected)
    }
}

pub struct ConfiguredEconomicsAdmissionSource {
    provider_key: String,
    inputs: AuthoritativeEconomicsInputStore,
    policy: ConfiguredEconomicsSourcePolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfiguredEconomicsSourcePolicy {
    pub quote_refresh_ns: u64,
    pub quote_max_age_ns: u64,
    pub quote_validity_ns: u64,
    pub resting_order_refresh_margin_ns: u64,
}

impl ConfiguredEconomicsAdmissionSource {
    pub fn new(
        provider_key: &str,
        inputs: AuthoritativeEconomicsInputStore,
        policy: ConfiguredEconomicsSourcePolicy,
    ) -> Result<Self, EconomicsUnavailable> {
        if provider_key.trim().is_empty()
            || policy.quote_refresh_ns == 0
            || policy.quote_max_age_ns == 0
            || policy.quote_validity_ns == 0
            || policy.resting_order_refresh_margin_ns == 0
            || policy.resting_order_refresh_margin_ns >= policy.quote_validity_ns
        {
            return Err(EconomicsUnavailable::InvalidQuoteValidityPolicy);
        }
        Ok(Self {
            provider_key: provider_key.to_string(),
            inputs,
            policy,
        })
    }
}

impl EconomicsAdmissionSource for ConfiguredEconomicsAdmissionSource {
    fn resolve_product_surface(
        &self,
        execution_client_id: &crate::economics::ExecutionClientId,
        instrument_id: &crate::economics::InstrumentId,
        candidates: &[crate::economics::ProductSurfaceId],
    ) -> Result<crate::economics::ProductSurfaceId, EconomicsUnavailable> {
        self.inputs.resolve_product_surface(
            execution_client_id,
            instrument_id,
            &self.provider_key,
            candidates,
        )
    }

    fn quote_admission(
        &self,
        intent: EconomicsAdmissionQuoteIntent,
    ) -> Result<EconomicsAdmission, EconomicsUnavailable> {
        let dependencies = self.inputs.dependencies(&intent.request)?;
        if dependencies.provider_key != self.provider_key {
            return Err(EconomicsUnavailable::AmbiguousQuoteAuthority);
        }
        let refresh_deadline_ns = dependencies
            .refreshed_at_ns
            .checked_add(self.policy.quote_refresh_ns)
            .ok_or(EconomicsUnavailable::InvalidQuoteValidityPolicy)?;
        let maximum_age_deadline_ns = dependencies
            .refreshed_at_ns
            .checked_add(self.policy.quote_max_age_ns)
            .ok_or(EconomicsUnavailable::InvalidQuoteValidityPolicy)?;
        if dependencies.refreshed_at_ns > intent.request.requested_at_ns {
            return Err(EconomicsUnavailable::InvalidSourceTimeline {
                source_id: crate::economics::SourceId::new(self.provider_key.clone())?,
            });
        }
        if intent.request.requested_at_ns > refresh_deadline_ns
            || intent.request.requested_at_ns > maximum_age_deadline_ns
        {
            return Err(EconomicsUnavailable::StaleSource {
                source_id: crate::economics::SourceId::new(self.provider_key.clone())?,
            });
        }
        let requested_at_ns = intent.request.requested_at_ns;
        let requires_resting_margin = intent.request.liquidity_role
            == crate::economics::LiquidityRoleAssumption::GuaranteedMaker;
        let resolved_edge_basis = dependencies.adapter.resolve_edge_basis(&intent.request)?;
        if resolved_edge_basis.normalized_amount != intent.base_reservation_notional
            || resolved_edge_basis.source_snapshot_ids.is_empty()
            || resolved_edge_basis.source_snapshot_ids
                != dependencies.edge_basis.source_snapshot_ids
        {
            return Err(EconomicsUnavailable::InvalidEdgeBasis);
        }
        let edge_basis = EdgeBasisEvidence {
            policy_id: intent.request.edge_basis_policy_id.clone(),
            resolver_id: dependencies.edge_basis.resolver_id,
            product_metadata_source: dependencies.edge_basis.product_metadata_source,
            policy_version: dependencies.edge_basis.policy_version,
            normalized_amount: resolved_edge_basis.normalized_amount,
            scope: crate::economics::EconomicScope::Decision {
                decision_correlation_id: intent.request.decision_correlation_id.clone(),
            },
            source_snapshot_ids: resolved_edge_basis.source_snapshot_ids,
            valid_until_ns: resolved_edge_basis
                .valid_until_ns
                .min(dependencies.edge_basis.valid_until_ns),
        };
        let admission = BoltV3EconomicsRuntime::from_offline_adapter(
            dependencies.adapter,
            self.policy.quote_validity_ns,
        )?
        .quote_admission(EconomicsAdmissionIntent {
            request: intent.request,
            order_binding: intent.order_binding,
            purpose: intent.purpose,
            gross_expected_value: intent.gross_expected_value,
            edge_basis,
            valuation_provider: dependencies.valuation_provider,
            base_reservation_notional: intent.base_reservation_notional,
        })?;
        if requires_resting_margin {
            let required_valid_until_ns = requested_at_ns
                .checked_add(self.policy.resting_order_refresh_margin_ns)
                .ok_or(EconomicsUnavailable::InvalidQuoteValidityPolicy)?;
            if admission.quote().valid_until_ns() < required_valid_until_ns {
                return Err(EconomicsUnavailable::StaleSource {
                    source_id: crate::economics::SourceId::new(self.provider_key.clone())?,
                });
            }
        }
        Ok(admission)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EconomicsAdmission {
    request: EconomicQuoteRequest,
    order_binding: EconomicsOrderBinding,
    purpose: EconomicsAdmissionPurpose,
    quote: EconomicQuote,
    net_edge: NetEdgeQuote,
    base_reservation_notional: Decimal,
    reservation_notional: Decimal,
    source_snapshot_ids: Vec<SnapshotId>,
}

impl EconomicsAdmission {
    pub fn request(&self) -> &EconomicQuoteRequest {
        &self.request
    }

    pub fn order_binding(&self) -> &EconomicsOrderBinding {
        &self.order_binding
    }

    pub fn purpose(&self) -> EconomicsAdmissionPurpose {
        self.purpose
    }

    pub fn quote(&self) -> &EconomicQuote {
        &self.quote
    }

    pub fn net_edge(&self) -> &NetEdgeQuote {
        &self.net_edge
    }

    pub fn reservation_notional(&self) -> Decimal {
        self.reservation_notional
    }

    pub fn base_reservation_notional(&self) -> Decimal {
        self.base_reservation_notional
    }

    pub fn debit_reservation(&self) -> Decimal {
        self.reservation_notional - self.base_reservation_notional
    }

    pub fn source_snapshot_ids(&self) -> &[SnapshotId] {
        &self.source_snapshot_ids
    }
}

pub struct BoltV3EconomicsRuntime {
    adapter: Arc<dyn VenueEconomicsAdapter>,
    quote_validity_ns: u64,
}

impl BoltV3EconomicsRuntime {
    pub fn from_offline_adapter(
        adapter: Arc<dyn VenueEconomicsAdapter>,
        quote_validity_ns: u64,
    ) -> Result<Self, EconomicsUnavailable> {
        if quote_validity_ns == 0 {
            return Err(EconomicsUnavailable::InvalidQuoteValidityPolicy);
        }
        Ok(Self {
            adapter,
            quote_validity_ns,
        })
    }

    pub fn quote_admission(
        &self,
        intent: EconomicsAdmissionIntent,
    ) -> Result<EconomicsAdmission, EconomicsUnavailable> {
        if intent.base_reservation_notional <= Decimal::ZERO {
            return Err(EconomicsUnavailable::InvalidPlannedFill);
        }
        let estimate = self.adapter.quote(&intent.request)?;
        let authority_snapshot_id = estimate.authority.snapshot_id.clone();
        let dependency_snapshot_ids = estimate
            .dependency_sources
            .iter()
            .map(|source| source.snapshot_id.clone())
            .collect::<Vec<_>>();
        let valuation_request = ValuationRequest {
            reporting_unit: intent.request.reporting_unit.clone(),
            reporting_policy_id: intent.request.reporting_policy_id.clone(),
            requested_at_ns: intent.request.requested_at_ns,
        };
        let mut valuations = Vec::new();
        for component in &estimate.components {
            if let Some(point_effect) = component.point_estimate.effect() {
                push_valuation(
                    &mut valuations,
                    intent.valuation_provider.as_ref(),
                    point_effect,
                    &valuation_request,
                )?;
            }
            if let Some(bound) = &component.debit_risk_bound {
                push_valuation(
                    &mut valuations,
                    intent.valuation_provider.as_ref(),
                    bound,
                    &valuation_request,
                )?;
            }
        }
        let mut quote = validate_and_aggregate_quote(&intent.request, estimate, &valuations)?;
        let configured_valid_until_ns = intent
            .request
            .requested_at_ns
            .checked_add(self.quote_validity_ns)
            .ok_or(EconomicsUnavailable::InvalidPlannedFill)?;
        quote.cap_valid_until_ns(configured_valid_until_ns);
        let net_edge = fold_net_edge(intent.gross_expected_value, &quote, intent.edge_basis)?;
        if intent.purpose == EconomicsAdmissionPurpose::TradingEdge
            && net_edge.core_net_edge() <= Decimal::ZERO
        {
            return Err(EconomicsUnavailable::NonPositiveNetEdge);
        }
        let debit_reservation = (-quote.core_total()).max(Decimal::ZERO);
        let reservation_notional = intent.base_reservation_notional + debit_reservation;
        let mut source_snapshot_ids = vec![authority_snapshot_id];
        source_snapshot_ids.extend(dependency_snapshot_ids);
        source_snapshot_ids.extend(
            quote
                .components()
                .iter()
                .map(|component| component.source.snapshot_id.clone()),
        );
        source_snapshot_ids.extend(
            quote
                .normalizations()
                .iter()
                .flat_map(|normalization| normalization.source_snapshot_ids.iter().cloned()),
        );
        source_snapshot_ids.extend(net_edge.basis().source_snapshot_ids.iter().cloned());
        source_snapshot_ids.sort();
        source_snapshot_ids.dedup();
        Ok(EconomicsAdmission {
            request: intent.request,
            order_binding: intent.order_binding,
            purpose: intent.purpose,
            quote,
            net_edge,
            base_reservation_notional: intent.base_reservation_notional,
            reservation_notional,
            source_snapshot_ids,
        })
    }
}

fn push_valuation(
    valuations: &mut Vec<crate::economics::ValuationEvidence>,
    provider: &dyn ValuationProvider,
    effect: &SignedNativeEffect,
    request: &ValuationRequest,
) -> Result<(), EconomicsUnavailable> {
    if valuations.iter().any(|evidence| {
        evidence.native_effect == *effect && evidence.reporting_unit == request.reporting_unit
    }) {
        return Ok(());
    }
    valuations.push(provider.value(effect, request)?);
    Ok(())
}

#[cfg(test)]
pub(crate) fn test_economics_admission(base_reservation_notional: Decimal) -> EconomicsAdmission {
    test_economics_admission_with_binding_and_purpose(
        base_reservation_notional,
        EconomicsOrderBinding::from_sha256(<sha2::Sha256 as sha2::Digest>::digest(
            b"test-order-binding",
        )),
        EconomicsAdmissionPurpose::TradingEdge,
    )
}

#[cfg(test)]
pub(crate) fn test_risk_reduction_economics_admission(
    base_reservation_notional: Decimal,
) -> EconomicsAdmission {
    test_economics_admission_with_binding_and_purpose(
        base_reservation_notional,
        EconomicsOrderBinding::from_sha256(<sha2::Sha256 as sha2::Digest>::digest(
            b"test-order-binding",
        )),
        EconomicsAdmissionPurpose::RiskReduction,
    )
}

#[cfg(test)]
pub(crate) fn test_economics_admission_with_binding(
    base_reservation_notional: Decimal,
    order_binding: EconomicsOrderBinding,
) -> EconomicsAdmission {
    test_economics_admission_with_binding_and_purpose(
        base_reservation_notional,
        order_binding,
        EconomicsAdmissionPurpose::TradingEdge,
    )
}

#[cfg(test)]
pub(crate) fn test_risk_reduction_economics_admission_with_binding(
    base_reservation_notional: Decimal,
    order_binding: EconomicsOrderBinding,
) -> EconomicsAdmission {
    test_economics_admission_with_binding_and_purpose(
        base_reservation_notional,
        order_binding,
        EconomicsAdmissionPurpose::RiskReduction,
    )
}

#[cfg(test)]
fn test_economics_admission_with_binding_and_purpose(
    base_reservation_notional: Decimal,
    order_binding: EconomicsOrderBinding,
    purpose: EconomicsAdmissionPurpose,
) -> EconomicsAdmission {
    use crate::economics::{
        AccountId, AdmissionTreatment, DecisionCorrelationId, EconomicClass, EconomicComponentId,
        EconomicKind, EconomicQuoteRequest, EconomicScope, EdgeBasisPolicyId,
        EstimatedEconomicComponent, ExecutionClientId, ExecutionKind, FormulaId, InstrumentId,
        LifecyclePath, LiquidityRoleAssumption, NativeUnitId, OrderSide, PlannedFillLeg,
        PointEstimate, ProductSurfaceId, ReportingPolicyId, RoutingContext, SignedNativeEffect,
        SourceId, SourceValidity, VenueQuoteEstimate,
    };

    #[derive(Clone)]
    struct TestAdapter(VenueQuoteEstimate);

    impl VenueEconomicsAdapter for TestAdapter {
        fn resolve_edge_basis(
            &self,
            request: &EconomicQuoteRequest,
        ) -> Result<crate::economics::ResolvedEdgeBasis, EconomicsUnavailable> {
            Ok(crate::economics::ResolvedEdgeBasis {
                normalized_amount: request
                    .planned_fill_legs
                    .iter()
                    .map(|leg| leg.price * leg.quantity)
                    .sum(),
                source_snapshot_ids: vec![self.0.authority.snapshot_id.clone()],
                valid_until_ns: self.0.authority.valid_until_ns,
            })
        }

        fn quote(
            &self,
            _request: &EconomicQuoteRequest,
        ) -> Result<VenueQuoteEstimate, EconomicsUnavailable> {
            Ok(self.0.clone())
        }
    }

    let requested_at_ns = 1;
    let valid_until_ns = u64::MAX;
    let reporting_unit = NativeUnitId::new("test-reporting-unit").expect("valid test unit");
    let decision_correlation_id =
        DecisionCorrelationId::new("test-decision").expect("valid test decision id");
    let source = SourceValidity {
        source_id: SourceId::new("test-economics-source").expect("valid test source id"),
        snapshot_id: SnapshotId::new("test-economics-snapshot").expect("valid test snapshot id"),
        source_at_ns: requested_at_ns,
        fetched_at_ns: requested_at_ns,
        valid_until_ns,
    };
    let request = EconomicQuoteRequest {
        execution_client_id: ExecutionClientId::new("test-execution-client")
            .expect("valid test execution client id"),
        account_id: AccountId::new("test-account").expect("valid test account id"),
        instrument_id: InstrumentId::new("test-instrument").expect("valid test instrument id"),
        product_surface_id: ProductSurfaceId::new("test-product-surface")
            .expect("valid test product surface id"),
        order_side: OrderSide::Buy,
        liquidity_role: LiquidityRoleAssumption::Taker,
        planned_fill_legs: vec![PlannedFillLeg {
            price: Decimal::ONE,
            quantity: base_reservation_notional,
        }],
        routing: RoutingContext {
            attached_charge: None,
        },
        position: None,
        lifecycle_path: LifecyclePath::PlannedExit,
        reporting_policy_id: ReportingPolicyId::new("test-reporting-policy")
            .expect("valid test reporting policy id"),
        reporting_unit: reporting_unit.clone(),
        edge_basis_policy_id: EdgeBasisPolicyId::new("test-edge-policy")
            .expect("valid test edge policy id"),
        requested_at_ns,
        decision_correlation_id: decision_correlation_id.clone(),
    };
    let adapter = TestAdapter(VenueQuoteEstimate {
        authority: source.clone(),
        dependency_sources: Vec::new(),
        components: vec![EstimatedEconomicComponent {
            component_id: EconomicComponentId::new("test-core-credit")
                .expect("valid test component id"),
            class: EconomicClass::Credit,
            kind: EconomicKind::Execution(ExecutionKind::ProtocolTrading),
            scope: EconomicScope::Decision {
                decision_correlation_id: decision_correlation_id.clone(),
            },
            point_estimate: PointEstimate::NonZero(
                SignedNativeEffect::currency(Decimal::ONE, reporting_unit)
                    .expect("valid test effect"),
            ),
            debit_risk_bound: None,
            admission_treatment: AdmissionTreatment::GuaranteedConditionalOnAction,
            calculation_factors: Vec::new(),
            formula_id: FormulaId::new("test-credit-formula").expect("valid test formula id"),
            source: source.clone(),
            normalized: None,
        }],
    });
    BoltV3EconomicsRuntime::from_offline_adapter(
        Arc::new(adapter),
        valid_until_ns - requested_at_ns,
    )
    .expect("test economics runtime policy should be valid")
    .quote_admission(EconomicsAdmissionIntent {
        request,
        order_binding,
        purpose,
        gross_expected_value: Decimal::ONE,
        edge_basis: EdgeBasisEvidence {
            policy_id: EdgeBasisPolicyId::new("test-edge-policy")
                .expect("valid test edge policy id"),
            resolver_id: FormulaId::new("test-edge-resolver").expect("valid test edge resolver id"),
            product_metadata_source: SourceId::new("test-product-metadata")
                .expect("valid test product metadata source"),
            policy_version: 1,
            normalized_amount: base_reservation_notional,
            scope: EconomicScope::Decision {
                decision_correlation_id,
            },
            source_snapshot_ids: vec![source.snapshot_id],
            valid_until_ns,
        },
        valuation_provider: identity_valuation_provider(),
        base_reservation_notional,
    })
    .expect("test economics admission should quote")
}

#[cfg(test)]
struct TestEconomicsAdmissionSource;

#[cfg(test)]
impl EconomicsAdmissionSource for TestEconomicsAdmissionSource {
    fn resolve_product_surface(
        &self,
        _execution_client_id: &crate::economics::ExecutionClientId,
        _instrument_id: &crate::economics::InstrumentId,
        candidates: &[crate::economics::ProductSurfaceId],
    ) -> Result<crate::economics::ProductSurfaceId, EconomicsUnavailable> {
        match candidates {
            [candidate] => Ok(candidate.clone()),
            [] => Err(EconomicsUnavailable::MissingQuoteAuthority),
            _ => Err(EconomicsUnavailable::AmbiguousQuoteAuthority),
        }
    }

    fn quote_admission(
        &self,
        intent: EconomicsAdmissionQuoteIntent,
    ) -> Result<EconomicsAdmission, EconomicsUnavailable> {
        use crate::economics::{
            AdmissionTreatment, EconomicClass, EconomicComponentId, EconomicKind, EconomicScope,
            EstimatedEconomicComponent, ExecutionKind, FormulaId, PointEstimate,
            SignedNativeEffect, SourceId, SourceValidity, VenueQuoteEstimate,
        };

        #[derive(Clone)]
        struct TestAdapter(VenueQuoteEstimate);

        impl VenueEconomicsAdapter for TestAdapter {
            fn resolve_edge_basis(
                &self,
                request: &EconomicQuoteRequest,
            ) -> Result<crate::economics::ResolvedEdgeBasis, EconomicsUnavailable> {
                Ok(crate::economics::ResolvedEdgeBasis {
                    normalized_amount: request
                        .planned_fill_legs
                        .iter()
                        .map(|leg| leg.price * leg.quantity)
                        .sum(),
                    source_snapshot_ids: vec![self.0.authority.snapshot_id.clone()],
                    valid_until_ns: self.0.authority.valid_until_ns,
                })
            }

            fn quote(
                &self,
                _request: &EconomicQuoteRequest,
            ) -> Result<VenueQuoteEstimate, EconomicsUnavailable> {
                Ok(self.0.clone())
            }
        }

        let valid_until_ns = u64::MAX;
        let source = SourceValidity {
            source_id: SourceId::new("test-economics-source")?,
            snapshot_id: SnapshotId::new("test-economics-snapshot")?,
            source_at_ns: intent.request.requested_at_ns,
            fetched_at_ns: intent.request.requested_at_ns,
            valid_until_ns,
        };
        let adapter = TestAdapter(VenueQuoteEstimate {
            authority: source.clone(),
            dependency_sources: Vec::new(),
            components: vec![EstimatedEconomicComponent {
                component_id: EconomicComponentId::new("test-core-credit")?,
                class: EconomicClass::Credit,
                kind: EconomicKind::Execution(ExecutionKind::ProtocolTrading),
                scope: EconomicScope::Decision {
                    decision_correlation_id: intent.request.decision_correlation_id.clone(),
                },
                point_estimate: PointEstimate::NonZero(SignedNativeEffect::currency(
                    Decimal::ONE,
                    intent.request.reporting_unit.clone(),
                )?),
                debit_risk_bound: None,
                admission_treatment: AdmissionTreatment::GuaranteedConditionalOnAction,
                calculation_factors: Vec::new(),
                formula_id: FormulaId::new("test-credit-formula")?,
                source: source.clone(),
                normalized: None,
            }],
        });
        BoltV3EconomicsRuntime::from_offline_adapter(
            Arc::new(adapter),
            valid_until_ns
                .checked_sub(intent.request.requested_at_ns)
                .ok_or(EconomicsUnavailable::InvalidQuoteValidityPolicy)?,
        )?
        .quote_admission(EconomicsAdmissionIntent {
            edge_basis: EdgeBasisEvidence {
                policy_id: intent.request.edge_basis_policy_id.clone(),
                resolver_id: FormulaId::new("test-edge-resolver")?,
                product_metadata_source: SourceId::new("test-product-metadata")?,
                policy_version: 1,
                normalized_amount: intent.base_reservation_notional,
                scope: EconomicScope::Decision {
                    decision_correlation_id: intent.request.decision_correlation_id.clone(),
                },
                source_snapshot_ids: vec![source.snapshot_id],
                valid_until_ns,
            },
            request: intent.request,
            order_binding: intent.order_binding,
            purpose: intent.purpose,
            gross_expected_value: intent.gross_expected_value,
            valuation_provider: identity_valuation_provider(),
            base_reservation_notional: intent.base_reservation_notional,
        })
    }
}

#[cfg(test)]
pub(crate) fn test_order_routing_handle(
    execution_client_id: &str,
) -> crate::bolt_v3_order_execution::BoltV3OrderRoutingHandle {
    crate::bolt_v3_order_execution::BoltV3OrderRoutingHandle::new(
        Arc::new(TestEconomicsAdmissionSource),
        crate::bolt_v3_order_execution::BoltV3OrderRoutingConfig {
            execution_client_id,
            account_id: "test-account",
            product_surface_id: "test-product-surface",
            reporting_policy_id: "test-reporting-policy",
            reporting_unit: "test-reporting-unit",
            edge_basis_policy_id: "test-edge-policy",
            carry_plan: crate::bolt_v3_order_execution::BoltV3CarryPlan::NoCarry,
            routing_attachment_policy:
                crate::bolt_v3_economics_config::EconomicsRoutingAttachmentPolicy::Forbidden,
        },
    )
    .expect("test order routing handle should build")
}
