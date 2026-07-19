use std::{str::FromStr, sync::Arc};

use bolt_v2::bolt_v3_economics_runtime::{
    AuthoritativeValuationObservation, BoltV3EconomicsRuntime, ConfiguredEconomicsSourcePolicy,
    ConfiguredValuationProvider, EconomicsAdmission, EconomicsAdmissionIntent,
    EconomicsAdmissionQuoteIntent, EconomicsAdmissionSource,
};
use bolt_v2::bolt_v3_providers::{
    OfflineEconomicsAdapterBuildContext, OfflineEconomicsSnapshotInput,
    build_offline_economics_adapter,
};
use bolt_v2::economics::{
    AccountId, DecisionCorrelationId, EconomicQuoteRequest, EconomicScope, EconomicsUnavailable,
    EdgeBasisEvidence, EdgeBasisPolicyId, ExecutionClientId, FormulaId, InstrumentId,
    LifecyclePath, LiquidityRoleAssumption, NativeUnitId, OrderSide, PlannedFillLeg,
    PositionContext, ProductSurfaceId, ReportingPolicyId, RoutingContext, SnapshotId, SourceId,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoricalEconomicsSnapshot {
    pub provider_key: String,
    pub execution_client_id: String,
    pub account_id: String,
    pub instrument_id: String,
    pub raw_symbol: String,
    pub product_surface_id: String,
    pub reporting_policy_id: String,
    pub reporting_unit: String,
    pub snapshot_id: String,
    pub source_id: String,
    pub source_at_ns: u64,
    pub fetched_at_ns: u64,
    pub valid_until_ns: u64,
    pub economics_toml: String,
    pub edge_basis: HistoricalEdgeBasisEvidence,
    pub source_snapshots: Vec<HistoricalSourceSnapshot>,
    pub valuation_observations: Vec<HistoricalValuationObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoricalEdgeBasisEvidence {
    pub policy_id: String,
    pub resolver_id: String,
    pub product_metadata_source: String,
    pub policy_version: u64,
    pub source_snapshot_ids: Vec<String>,
    pub valid_until_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoricalSourceSnapshot {
    pub source_id: String,
    pub snapshot_id: String,
    pub source_at_ns: u64,
    pub fetched_at_ns: u64,
    pub valid_until_ns: u64,
    pub payload_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "authority", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoricalValuationObservation {
    MarketQuote {
        client_id: String,
        instrument_id: String,
        price: String,
        snapshot_id: String,
        observed_at_ns: u64,
        fetched_at_ns: u64,
        valid_until_ns: u64,
    },
    ProviderConversion {
        source_id: String,
        from_unit: String,
        to_unit: String,
        rate: String,
        snapshot_id: String,
        observed_at_ns: u64,
        fetched_at_ns: u64,
        valid_until_ns: u64,
    },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum HistoricalValuationAuthorityKey {
    MarketQuote {
        client_id: String,
        instrument_id: String,
    },
    ProviderConversion {
        source_id: String,
        from_unit: String,
        to_unit: String,
    },
}

pub struct ReplayEconomicsAdapter {
    snapshot: HistoricalEconomicsSnapshot,
    adapter: Arc<dyn bolt_v2::economics::VenueEconomicsAdapter>,
    economics: bolt_v2::bolt_v3_economics_config::ExecutionEconomicsConfig,
}

pub struct ReplayEconomicsAdmissionSource {
    snapshots: Vec<HistoricalEconomicsSnapshot>,
    resting_order_refresh_margin_ns: u64,
}

impl ReplayEconomicsAdmissionSource {
    pub fn from_snapshots(
        snapshots: Vec<HistoricalEconomicsSnapshot>,
    ) -> Result<Self, EconomicsUnavailable> {
        if snapshots.is_empty() {
            return Err(EconomicsUnavailable::MissingQuoteAuthority);
        }
        let authority = &snapshots[0];
        if snapshots.iter().any(|snapshot| {
            snapshot.provider_key != authority.provider_key
                || snapshot.execution_client_id != authority.execution_client_id
                || snapshot.account_id != authority.account_id
                || snapshot.reporting_policy_id != authority.reporting_policy_id
                || snapshot.reporting_unit != authority.reporting_unit
        }) {
            return Err(EconomicsUnavailable::AmbiguousQuoteAuthority);
        }
        let mut configured_policy = None;
        for snapshot in &snapshots {
            validate_snapshot(snapshot)?;
            let adapter = ReplayEconomicsAdapter::from_snapshot(snapshot.clone())?;
            let policy = (
                adapter.economics.quote_refresh_secs,
                adapter.economics.quote_max_age_secs,
                adapter.economics.quote_validity_ms,
                adapter.economics.resting_order_refresh_margin_ms,
            );
            match configured_policy {
                Some(existing) if existing != policy => {
                    return Err(EconomicsUnavailable::AmbiguousQuoteAuthority);
                }
                Some(_) => {}
                None => configured_policy = Some(policy),
            }
        }
        let (_, _, _, resting_order_refresh_margin_ms) = configured_policy
            .ok_or(EconomicsUnavailable::MissingQuoteAuthority)?;
        let resting_order_refresh_margin_ns = resting_order_refresh_margin_ms
            .checked_mul(1_000_000)
            .ok_or(EconomicsUnavailable::InvalidQuoteValidityPolicy)?;
        Ok(Self {
            snapshots,
            resting_order_refresh_margin_ns,
        })
    }

    pub fn snapshot_for_request(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<&HistoricalEconomicsSnapshot, EconomicsUnavailable> {
        let matching = self
            .snapshots
            .iter()
            .filter(|snapshot| snapshot_matches_request(snapshot, request))
            .collect::<Vec<_>>();
        if matching.is_empty() {
            return Err(EconomicsUnavailable::MissingQuoteAuthority);
        }
        let eligible = matching.iter().copied().filter(|snapshot| {
            snapshot.source_at_ns <= request.requested_at_ns
                && snapshot.fetched_at_ns <= request.requested_at_ns
                && request.requested_at_ns <= snapshot.valid_until_ns
        });
        let Some(latest_activation) = eligible
            .clone()
            .map(|snapshot| (snapshot.fetched_at_ns, snapshot.source_at_ns))
            .max()
        else {
            let source_id = SourceId::new(matching[0].source_id.clone())?;
            if matching.iter().any(|snapshot| {
                snapshot.source_at_ns > request.requested_at_ns
                    || snapshot.fetched_at_ns > request.requested_at_ns
            }) {
                return Err(EconomicsUnavailable::InvalidSourceTimeline { source_id });
            }
            return Err(EconomicsUnavailable::StaleSource { source_id });
        };
        let selected = eligible
            .filter(|snapshot| (snapshot.fetched_at_ns, snapshot.source_at_ns) == latest_activation)
            .collect::<Vec<_>>();
        match selected.as_slice() {
            [snapshot] => Ok(*snapshot),
            _ => Err(EconomicsUnavailable::AmbiguousQuoteAuthority),
        }
    }

    pub fn order_routing_handle(
        self,
    ) -> Result<bolt_v2::bolt_v3_order_execution::BoltV3OrderRoutingHandle, EconomicsUnavailable>
    {
        let authority = self.snapshots[0].clone();
        let mut route_configs = std::collections::BTreeMap::new();
        let mut routing_attachment_policy = None;
        for snapshot in &self.snapshots {
            let adapter = ReplayEconomicsAdapter::from_snapshot(snapshot.clone())?;
            match routing_attachment_policy {
                Some(policy) if policy != adapter.economics.routing_attachment_policy => {
                    return Err(EconomicsUnavailable::AmbiguousQuoteAuthority);
                }
                Some(_) => {}
                None => {
                    routing_attachment_policy = Some(adapter.economics.routing_attachment_policy);
                }
            }
            let carry_plan = if adapter
                .economics
                .carry_surfaces
                .contains(&snapshot.product_surface_id)
            {
                bolt_v2::bolt_v3_order_execution::BoltV3CarryPlan::Required
            } else {
                bolt_v2::bolt_v3_order_execution::BoltV3CarryPlan::NoCarry
            };
            let route = (snapshot.edge_basis.policy_id.clone(), carry_plan);
            if let Some(existing) =
                route_configs.insert(snapshot.product_surface_id.clone(), route.clone())
                && existing != route
            {
                return Err(EconomicsUnavailable::AmbiguousQuoteAuthority);
            }
        }
        let routes = route_configs
            .iter()
            .map(|(surface, (policy, carry_plan))| {
                bolt_v2::bolt_v3_order_execution::BoltV3ProductSurfaceRoute {
                    product_surface_id: surface,
                    edge_basis_policy_id: policy,
                    carry_plan: *carry_plan,
                }
            })
            .collect();
        bolt_v2::bolt_v3_order_execution::BoltV3OrderRoutingHandle::new_with_product_surfaces(
            std::sync::Arc::new(self),
            bolt_v2::bolt_v3_order_execution::BoltV3MultiSurfaceOrderRoutingConfig {
                execution_client_id: &authority.execution_client_id,
                account_id: &authority.account_id,
                product_surface_routes: routes,
                reporting_policy_id: &authority.reporting_policy_id,
                reporting_unit: &authority.reporting_unit,
                routing_attachment_policy: routing_attachment_policy
                    .ok_or(EconomicsUnavailable::MissingQuoteAuthority)?,
            },
        )
        .map_err(|_| EconomicsUnavailable::InvalidIdentifier {
            kind: "ReplayEconomicsAdmissionSource",
        })
    }
}

impl EconomicsAdmissionSource for ReplayEconomicsAdmissionSource {
    fn resting_order_refresh_margin_ns(&self) -> u64 {
        self.resting_order_refresh_margin_ns
    }

    fn resolve_product_surface(
        &self,
        execution_client_id: &ExecutionClientId,
        instrument_id: &InstrumentId,
        candidates: &[ProductSurfaceId],
    ) -> Result<ProductSurfaceId, EconomicsUnavailable> {
        let matching = self
            .snapshots
            .iter()
            .filter(|snapshot| {
                snapshot.execution_client_id == execution_client_id.as_str()
                    && snapshot.instrument_id == instrument_id.as_str()
                    && candidates
                        .iter()
                        .any(|candidate| candidate.as_str() == snapshot.product_surface_id)
            })
            .map(|snapshot| snapshot.product_surface_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        match matching.iter().copied().collect::<Vec<_>>().as_slice() {
            [] => Err(EconomicsUnavailable::MissingQuoteAuthority),
            [surface] => ProductSurfaceId::new(*surface),
            _ => Err(EconomicsUnavailable::AmbiguousQuoteAuthority),
        }
    }

    fn quote_admission(
        &self,
        intent: EconomicsAdmissionQuoteIntent,
    ) -> Result<EconomicsAdmission, EconomicsUnavailable> {
        let snapshot = self.snapshot_for_request(&intent.request)?;
        let adapter = ReplayEconomicsAdapter::from_snapshot(snapshot.clone())?;
        let planned_fill_notional =
            bolt_v2::economics::PlannedFillNotional::from_legs(
                &intent.request.planned_fill_legs,
            )?;
        let edge_basis = adapter.edge_basis(&intent.request, planned_fill_notional)?;
        let observations = canonical_valuation_observations(snapshot)?;
        let valuation_provider = Arc::new(ConfiguredValuationProvider::from_config(
            &adapter.economics.valuation,
            &observations,
        )?);
        let seconds_to_ns = |seconds: u64| {
            seconds
                .checked_mul(1_000_000_000)
                .ok_or(EconomicsUnavailable::InvalidQuoteValidityPolicy)
        };
        let millis_to_ns = |millis: u64| {
            millis
                .checked_mul(1_000_000)
                .ok_or(EconomicsUnavailable::InvalidQuoteValidityPolicy)
        };
        let policy = ConfiguredEconomicsSourcePolicy {
            quote_refresh_ns: seconds_to_ns(adapter.economics.quote_refresh_secs)?,
            quote_max_age_ns: seconds_to_ns(adapter.economics.quote_max_age_secs)?,
            quote_validity_ns: millis_to_ns(adapter.economics.quote_validity_ms)?,
            resting_order_refresh_margin_ns: millis_to_ns(
                adapter.economics.resting_order_refresh_margin_ms,
            )?,
        };
        let authority_refreshed_at_ns = snapshot.fetched_at_ns;
        BoltV3EconomicsRuntime::try_new(std::sync::Arc::new(adapter), policy)?.quote_admission(
            EconomicsAdmissionIntent {
                provider_key: snapshot.provider_key.clone(),
                request: intent.request,
                order_binding: intent.order_binding,
                purpose: intent.purpose,
                gross_expected_value: intent.gross_expected_value,
                edge_basis,
                planned_fill_notional,
                valuation_provider,
                reservation_basis: intent.reservation_basis,
                authority_refreshed_at_ns,
            },
        )
    }
}

impl ReplayEconomicsAdapter {
    pub fn from_snapshot(
        snapshot: HistoricalEconomicsSnapshot,
    ) -> Result<Self, EconomicsUnavailable> {
        validate_snapshot(&snapshot)?;
        let economics = toml::from_str::<toml::Value>(&snapshot.economics_toml)
            .map_err(|_| EconomicsUnavailable::MissingQuoteAuthority)?;
        let snapshots = snapshot
            .source_snapshots
            .iter()
            .map(|source| OfflineEconomicsSnapshotInput {
                source_id: source.source_id.clone(),
                snapshot_id: source.snapshot_id.clone(),
                source_at_ns: source.source_at_ns,
                fetched_at_ns: source.fetched_at_ns,
                valid_until_ns: source.valid_until_ns,
                payload: source.payload_json.as_bytes().to_vec(),
            })
            .collect::<Vec<_>>();
        let binding = build_offline_economics_adapter(
            &snapshot.provider_key,
            OfflineEconomicsAdapterBuildContext {
                account_id: &snapshot.account_id,
                instrument_id: &snapshot.instrument_id,
                raw_symbol: &snapshot.raw_symbol,
                economics: &economics,
                snapshots: &snapshots,
            },
        )
        .map_err(|_| EconomicsUnavailable::MissingQuoteAuthority)?;
        if binding.economics.product_surface_policies.len() != 1 {
            return Err(EconomicsUnavailable::AmbiguousQuoteAuthority);
        }
        let configured_edge_basis = binding
            .economics
            .edge_basis
            .get(&snapshot.edge_basis.policy_id)
            .ok_or(EconomicsUnavailable::EdgeBasisPolicyMismatch)?;
        if configured_edge_basis.resolver_id != snapshot.edge_basis.resolver_id
            || configured_edge_basis.product_metadata_source
                != snapshot.edge_basis.product_metadata_source
            || configured_edge_basis.policy_version != snapshot.edge_basis.policy_version
        {
            return Err(EconomicsUnavailable::InvalidEdgeBasis);
        }
        if binding.economics.reporting_policy != snapshot.reporting_policy_id {
            return Err(EconomicsUnavailable::MissingQuoteAuthority);
        }
        if binding
            .economics
            .product_surface_policies
            .get(&snapshot.product_surface_id)
            != Some(&snapshot.edge_basis.policy_id)
        {
            return Err(EconomicsUnavailable::EdgeBasisPolicyMismatch);
        }
        Ok(Self {
            snapshot,
            adapter: binding.adapter,
            economics: binding.economics,
        })
    }

    pub fn edge_basis(
        &self,
        request: &EconomicQuoteRequest,
        planned_fill_notional: bolt_v2::economics::PlannedFillNotional,
    ) -> Result<EdgeBasisEvidence, EconomicsUnavailable> {
        if self.snapshot.edge_basis.policy_id != request.edge_basis_policy_id.as_str() {
            return Err(EconomicsUnavailable::EdgeBasisPolicyMismatch);
        }
        if request.requested_at_ns > self.snapshot.edge_basis.valid_until_ns {
            return Err(EconomicsUnavailable::StaleEdgeBasis {
                valid_until_ns: self.snapshot.edge_basis.valid_until_ns,
            });
        }
        let resolved = self
            .adapter
            .resolve_edge_basis(request, planned_fill_notional)?;
        if resolved.source_snapshot_ids
            != self
                .snapshot
                .edge_basis
                .source_snapshot_ids
                .iter()
                .map(|value| SnapshotId::new(value.clone()))
                .collect::<Result<Vec<_>, _>>()?
        {
            return Err(EconomicsUnavailable::InvalidEdgeBasis);
        }
        Ok(EdgeBasisEvidence {
            policy_id: EdgeBasisPolicyId::new(self.snapshot.edge_basis.policy_id.clone())?,
            resolver_id: FormulaId::new(self.snapshot.edge_basis.resolver_id.clone())?,
            product_metadata_source: SourceId::new(
                self.snapshot.edge_basis.product_metadata_source.clone(),
            )?,
            policy_version: self.snapshot.edge_basis.policy_version,
            normalized_amount: resolved.normalized_amount,
            scope: EconomicScope::Decision {
                decision_correlation_id: request.decision_correlation_id.clone(),
            },
            source_snapshot_ids: self
                .snapshot
                .edge_basis
                .source_snapshot_ids
                .iter()
                .map(|value| SnapshotId::new(value.clone()))
                .collect::<Result<Vec<_>, _>>()?,
            valid_until_ns: resolved
                .valid_until_ns
                .min(self.snapshot.edge_basis.valid_until_ns),
        })
    }
}

impl bolt_v2::economics::VenueEconomicsAdapter for ReplayEconomicsAdapter {
    fn resolve_edge_basis(
        &self,
        request: &EconomicQuoteRequest,
        planned_fill_notional: bolt_v2::economics::PlannedFillNotional,
    ) -> Result<bolt_v2::economics::ResolvedEdgeBasis, EconomicsUnavailable> {
        validate_request_binding(&self.snapshot, request)?;
        self.adapter
            .resolve_edge_basis(request, planned_fill_notional)
    }

    fn quote(
        &self,
        request: &EconomicQuoteRequest,
        planned_fill_notional: bolt_v2::economics::PlannedFillNotional,
    ) -> Result<bolt_v2::economics::VenueQuoteEstimate, EconomicsUnavailable> {
        validate_request_binding(&self.snapshot, request)?;
        if request.requested_at_ns < self.snapshot.source_at_ns
            || request.requested_at_ns > self.snapshot.valid_until_ns
        {
            return Err(EconomicsUnavailable::StaleSource {
                source_id: SourceId::new(self.snapshot.source_id.clone())?,
            });
        }
        self.adapter.quote(request, planned_fill_notional)
    }
}

fn validate_snapshot(snapshot: &HistoricalEconomicsSnapshot) -> Result<(), EconomicsUnavailable> {
    if snapshot.source_at_ns > snapshot.fetched_at_ns
        || snapshot.fetched_at_ns > snapshot.valid_until_ns
        || snapshot.edge_basis.valid_until_ns < snapshot.source_at_ns
        || snapshot.provider_key.trim().is_empty()
        || snapshot.economics_toml.trim().is_empty()
        || snapshot.instrument_id.trim().is_empty()
        || snapshot.raw_symbol.trim().is_empty()
        || snapshot.source_snapshots.is_empty()
    {
        return Err(EconomicsUnavailable::InvalidSourceTimeline {
            source_id: SourceId::new(snapshot.source_id.clone())?,
        });
    }
    SourceId::new(snapshot.source_id.clone())?;
    ExecutionClientId::new(snapshot.execution_client_id.clone())?;
    AccountId::new(snapshot.account_id.clone())?;
    InstrumentId::new(snapshot.instrument_id.clone())?;
    ProductSurfaceId::new(snapshot.product_surface_id.clone())?;
    ReportingPolicyId::new(snapshot.reporting_policy_id.clone())?;
    NativeUnitId::new(snapshot.reporting_unit.clone())?;
    SnapshotId::new(snapshot.snapshot_id.clone())?;
    EdgeBasisPolicyId::new(snapshot.edge_basis.policy_id.clone())?;
    let mut edge_snapshot_ids = std::collections::BTreeSet::new();
    for snapshot_id in &snapshot.edge_basis.source_snapshot_ids {
        if !edge_snapshot_ids.insert(snapshot_id.clone()) {
            return Err(EconomicsUnavailable::InvalidQuoteValidityPolicy);
        }
        SnapshotId::new(snapshot_id.clone())?;
    }
    let mut source_ids = std::collections::BTreeSet::new();
    let mut source_snapshot_ids = std::collections::BTreeSet::new();
    for source in &snapshot.source_snapshots {
        if source.source_at_ns > source.fetched_at_ns
            || source.fetched_at_ns > source.valid_until_ns
            || source.source_at_ns > snapshot.source_at_ns
            || source.fetched_at_ns > snapshot.fetched_at_ns
            || source.valid_until_ns < snapshot.valid_until_ns
            || !source_ids.insert(source.source_id.clone())
            || !source_snapshot_ids.insert(source.snapshot_id.clone())
            || source.payload_json.trim().is_empty()
        {
            return Err(EconomicsUnavailable::InvalidSourceTimeline {
                source_id: SourceId::new(source.source_id.clone())?,
            });
        }
        SourceId::new(source.source_id.clone())?;
        SnapshotId::new(source.snapshot_id.clone())?;
    }
    let mut valuation_authorities = std::collections::BTreeSet::new();
    let mut valuation_snapshot_ids = std::collections::BTreeSet::new();
    for observation in &snapshot.valuation_observations {
        let (authority, snapshot_id, observed_at_ns, fetched_at_ns, valid_until_ns) =
            match observation {
                HistoricalValuationObservation::MarketQuote {
                    client_id,
                    instrument_id,
                    snapshot_id,
                    observed_at_ns,
                    fetched_at_ns,
                    valid_until_ns,
                    ..
                } => (
                    HistoricalValuationAuthorityKey::MarketQuote {
                        client_id: client_id.clone(),
                        instrument_id: instrument_id.clone(),
                    },
                    snapshot_id,
                    *observed_at_ns,
                    *fetched_at_ns,
                    *valid_until_ns,
                ),
                HistoricalValuationObservation::ProviderConversion {
                    source_id,
                    from_unit,
                    to_unit,
                    snapshot_id,
                    observed_at_ns,
                    fetched_at_ns,
                    valid_until_ns,
                    ..
                } => (
                    HistoricalValuationAuthorityKey::ProviderConversion {
                        source_id: source_id.clone(),
                        from_unit: from_unit.clone(),
                        to_unit: to_unit.clone(),
                    },
                    snapshot_id,
                    *observed_at_ns,
                    *fetched_at_ns,
                    *valid_until_ns,
                ),
            };
        if observed_at_ns > fetched_at_ns
            || fetched_at_ns > valid_until_ns
            || observed_at_ns > snapshot.source_at_ns
            || fetched_at_ns > snapshot.fetched_at_ns
            || valid_until_ns < snapshot.valid_until_ns
            || !valuation_authorities.insert(authority)
            || !valuation_snapshot_ids.insert(snapshot_id.clone())
        {
            return Err(EconomicsUnavailable::InvalidQuoteValidityPolicy);
        }
        SnapshotId::new(snapshot_id.clone())?;
    }
    Ok(())
}

fn snapshot_matches_request(
    snapshot: &HistoricalEconomicsSnapshot,
    request: &EconomicQuoteRequest,
) -> bool {
    snapshot.execution_client_id == request.execution_client_id.as_str()
        && snapshot.account_id == request.account_id.as_str()
        && snapshot.instrument_id == request.instrument_id.as_str()
        && snapshot.product_surface_id == request.product_surface_id.as_str()
        && snapshot.reporting_policy_id == request.reporting_policy_id.as_str()
        && snapshot.reporting_unit == request.reporting_unit.as_str()
        && snapshot.edge_basis.policy_id == request.edge_basis_policy_id.as_str()
}

fn validate_request_binding(
    snapshot: &HistoricalEconomicsSnapshot,
    request: &EconomicQuoteRequest,
) -> Result<(), EconomicsUnavailable> {
    if snapshot_matches_request(snapshot, request) {
        Ok(())
    } else {
        Err(EconomicsUnavailable::MissingQuoteAuthority)
    }
}

fn canonical_valuation_observations(
    snapshot: &HistoricalEconomicsSnapshot,
) -> Result<Vec<AuthoritativeValuationObservation>, EconomicsUnavailable> {
    snapshot
        .valuation_observations
        .iter()
        .map(|observation| match observation {
            HistoricalValuationObservation::MarketQuote {
                client_id,
                instrument_id,
                price,
                snapshot_id,
                observed_at_ns,
                fetched_at_ns,
                valid_until_ns,
            } => Ok(AuthoritativeValuationObservation::MarketQuote {
                client_id: client_id.clone(),
                instrument_id: instrument_id.clone(),
                price: decimal(price)?,
                snapshot_id: SnapshotId::new(snapshot_id.clone())?,
                observed_at_ns: *observed_at_ns,
                fetched_at_ns: *fetched_at_ns,
                valid_until_ns: *valid_until_ns,
            }),
            HistoricalValuationObservation::ProviderConversion {
                source_id,
                from_unit,
                to_unit,
                rate,
                snapshot_id,
                observed_at_ns,
                fetched_at_ns,
                valid_until_ns,
            } => Ok(AuthoritativeValuationObservation::ProviderConversion {
                source_id: source_id.clone(),
                from_unit: NativeUnitId::new(from_unit.clone())?,
                to_unit: NativeUnitId::new(to_unit.clone())?,
                rate: decimal(rate)?,
                snapshot_id: SnapshotId::new(snapshot_id.clone())?,
                observed_at_ns: *observed_at_ns,
                fetched_at_ns: *fetched_at_ns,
                valid_until_ns: *valid_until_ns,
            }),
        })
        .collect()
}

fn decimal(value: &str) -> Result<Decimal, EconomicsUnavailable> {
    Decimal::from_str(value).map_err(|_| EconomicsUnavailable::InvalidDecimal)
}

pub struct ReplayQuoteIntent<'a> {
    pub execution_client_id: &'a str,
    pub account_id: &'a str,
    pub instrument_id: &'a str,
    pub product_surface_id: &'a str,
    pub order_side: OrderSide,
    pub liquidity_role: LiquidityRoleAssumption,
    pub planned_fill_legs: Vec<PlannedFillLeg>,
    pub routing_attachment_id: Option<&'a str>,
    pub position: Option<PositionContext>,
    pub lifecycle_path: LifecyclePath,
    pub reporting_policy_id: &'a str,
    pub reporting_unit: &'a str,
    pub requested_at_ns: u64,
    pub decision_correlation_id: &'a str,
    pub edge_basis_policy_id: &'a str,
}

pub fn canonical_quote_request_from_replay(
    intent: ReplayQuoteIntent<'_>,
) -> Result<EconomicQuoteRequest, EconomicsUnavailable> {
    if intent.planned_fill_legs.is_empty()
        || intent
            .planned_fill_legs
            .iter()
            .any(|leg| leg.price <= Decimal::ZERO || leg.quantity <= Decimal::ZERO)
    {
        return Err(EconomicsUnavailable::InvalidPlannedFill);
    }
    Ok(EconomicQuoteRequest {
        execution_client_id: ExecutionClientId::new(intent.execution_client_id)?,
        account_id: AccountId::new(intent.account_id)?,
        instrument_id: InstrumentId::new(intent.instrument_id)?,
        product_surface_id: ProductSurfaceId::new(intent.product_surface_id)?,
        order_side: intent.order_side,
        liquidity_role: intent.liquidity_role,
        planned_fill_legs: intent.planned_fill_legs,
        routing: RoutingContext {
            attached_charge: intent
                .routing_attachment_id
                .map(|attachment_id| {
                    Ok(bolt_v2::economics::RoutingAttachment {
                        attachment_id: bolt_v2::economics::RoutingAttachmentId::new(attachment_id)?,
                    })
                })
                .transpose()?,
        },
        position: intent.position,
        lifecycle_path: intent.lifecycle_path,
        reporting_policy_id: ReportingPolicyId::new(intent.reporting_policy_id)?,
        reporting_unit: NativeUnitId::new(intent.reporting_unit)?,
        edge_basis_policy_id: EdgeBasisPolicyId::new(intent.edge_basis_policy_id)?,
        requested_at_ns: intent.requested_at_ns,
        decision_correlation_id: DecisionCorrelationId::new(intent.decision_correlation_id)?,
    })
}
