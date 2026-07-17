use std::str::FromStr;

use bolt_v2::bolt_v3_economics_runtime::{
    BoltV3EconomicsRuntime, EconomicsAdmission, EconomicsAdmissionIntent,
    EconomicsAdmissionQuoteIntent, EconomicsAdmissionSource,
};
use bolt_v2::economics::{
    AccountId, AdmissionTreatment, DecisionCorrelationId, EconomicClass, EconomicComponentId,
    EconomicKind, EconomicQuoteRequest, EconomicScope, EconomicsUnavailable, EdgeBasisEvidence,
    EdgeBasisPolicyId, EstimatedEconomicComponent, ExecutionClientId, ExecutionKind, FormulaId,
    InstrumentId, LifecyclePath, LiquidityRoleAssumption, NativeUnitId, OrderId, OrderSide,
    PlannedFillLeg, ProductSurfaceId, ReportingPolicyId, RiskBoundAuthority, RoutingContext,
    SignedNativeEffect, SnapshotId, SourceId, SourceValidity, ValuationEvidence,
    VenueEconomicsAdapter, VenueQuoteEstimate,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoricalEconomicsSnapshot {
    pub execution_client_id: String,
    pub account_id: String,
    pub product_surface_id: String,
    pub reporting_policy_id: String,
    pub reporting_unit: String,
    pub snapshot_id: String,
    pub source_id: String,
    pub source_at_ns: u64,
    pub fetched_at_ns: u64,
    pub valid_until_ns: u64,
    pub edge_basis: HistoricalEdgeBasisEvidence,
    pub components: Vec<HistoricalEconomicComponent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoricalEdgeBasisEvidence {
    pub policy_id: String,
    pub policy_version: u64,
    pub normalized_amount: String,
    pub source_snapshot_ids: Vec<String>,
    pub valid_until_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoricalEconomicComponent {
    pub component_id: String,
    pub order_id: String,
    pub class: HistoricalEconomicClass,
    pub treatment: HistoricalAdmissionTreatment,
    pub native_amount: String,
    pub native_unit: String,
    pub debit_risk_bound: Option<String>,
    pub formula_id: String,
    pub source_id: String,
    pub snapshot_id: String,
    pub source_at_ns: u64,
    pub fetched_at_ns: u64,
    pub valid_until_ns: u64,
    pub valuation: Option<HistoricalValuationEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoricalEconomicClass {
    Charge,
    Credit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoricalAdmissionTreatment {
    GuaranteedConditionalOnAction,
    VenueRiskBound,
    OperatorRiskBound,
    ForecastOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoricalValuationEvidence {
    pub normalized_amount: String,
    pub reporting_unit: String,
    pub route_id: Option<String>,
    pub source_snapshot_ids: Vec<String>,
    pub valued_at_ns: u64,
    pub valid_until_ns: Option<u64>,
}

pub struct ReplayEconomicsAdapter {
    snapshot: HistoricalEconomicsSnapshot,
}

pub struct ReplayEconomicsAdmissionSource {
    snapshots: Vec<HistoricalEconomicsSnapshot>,
}

impl ReplayEconomicsAdmissionSource {
    pub fn from_snapshots(
        snapshots: Vec<HistoricalEconomicsSnapshot>,
    ) -> Result<Self, EconomicsUnavailable> {
        if snapshots.is_empty() {
            return Err(EconomicsUnavailable::MissingQuoteAuthority);
        }
        for snapshot in &snapshots {
            validate_snapshot(snapshot)?;
        }
        let authority = &snapshots[0];
        if snapshots.iter().any(|snapshot| {
            snapshot.execution_client_id != authority.execution_client_id
                || snapshot.account_id != authority.account_id
                || snapshot.product_surface_id != authority.product_surface_id
                || snapshot.reporting_policy_id != authority.reporting_policy_id
                || snapshot.reporting_unit != authority.reporting_unit
                || snapshot.edge_basis.policy_id != authority.edge_basis.policy_id
        }) {
            return Err(EconomicsUnavailable::AmbiguousQuoteAuthority);
        }
        Ok(Self { snapshots })
    }

    pub fn order_routing_handle(
        self,
    ) -> Result<bolt_v2::bolt_v3_order_execution::BoltV3OrderRoutingHandle, EconomicsUnavailable>
    {
        let authority = self.snapshots[0].clone();
        bolt_v2::bolt_v3_order_execution::BoltV3OrderRoutingHandle::new(
            std::sync::Arc::new(self),
            &authority.execution_client_id,
            &authority.account_id,
            &authority.product_surface_id,
            &authority.reporting_policy_id,
            &authority.reporting_unit,
            &authority.edge_basis.policy_id,
        )
        .map_err(|_| EconomicsUnavailable::InvalidIdentifier {
            kind: "ReplayEconomicsAdmissionSource",
        })
    }
}

impl EconomicsAdmissionSource for ReplayEconomicsAdmissionSource {
    fn quote_admission(
        &self,
        intent: EconomicsAdmissionQuoteIntent,
    ) -> Result<EconomicsAdmission, EconomicsUnavailable> {
        let matching = self
            .snapshots
            .iter()
            .filter(|snapshot| {
                snapshot.source_at_ns <= intent.request.requested_at_ns
                    && intent.request.requested_at_ns <= snapshot.valid_until_ns
            })
            .collect::<Vec<_>>();
        let snapshot = match matching.as_slice() {
            [] => return Err(EconomicsUnavailable::MissingQuoteAuthority),
            [snapshot] => *snapshot,
            _ => return Err(EconomicsUnavailable::AmbiguousQuoteAuthority),
        };
        let adapter = ReplayEconomicsAdapter::from_snapshot(snapshot.clone())?;
        let edge_basis = adapter.edge_basis(&intent.request)?;
        BoltV3EconomicsRuntime::from_offline_adapter(std::sync::Arc::new(adapter)).quote_admission(
            EconomicsAdmissionIntent {
                request: intent.request,
                gross_expected_value: intent.gross_expected_value,
                edge_basis,
                valuations: Vec::new(),
                base_reservation_notional: intent.base_reservation_notional,
            },
        )
    }
}

impl ReplayEconomicsAdapter {
    pub fn from_snapshot(
        snapshot: HistoricalEconomicsSnapshot,
    ) -> Result<Self, EconomicsUnavailable> {
        validate_snapshot(&snapshot)?;
        Ok(Self { snapshot })
    }

    pub fn edge_basis(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<EdgeBasisEvidence, EconomicsUnavailable> {
        if self.snapshot.edge_basis.policy_id != request.edge_basis_policy_id.as_str() {
            return Err(EconomicsUnavailable::EdgeBasisPolicyMismatch);
        }
        if request.requested_at_ns > self.snapshot.edge_basis.valid_until_ns {
            return Err(EconomicsUnavailable::StaleEdgeBasis {
                valid_until_ns: self.snapshot.edge_basis.valid_until_ns,
            });
        }
        Ok(EdgeBasisEvidence {
            policy_id: EdgeBasisPolicyId::new(self.snapshot.edge_basis.policy_id.clone())?,
            policy_version: self.snapshot.edge_basis.policy_version,
            normalized_amount: decimal(&self.snapshot.edge_basis.normalized_amount)?,
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
            valid_until_ns: self.snapshot.edge_basis.valid_until_ns,
        })
    }
}

impl VenueEconomicsAdapter for ReplayEconomicsAdapter {
    fn quote(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<VenueQuoteEstimate, EconomicsUnavailable> {
        if request.requested_at_ns < self.snapshot.source_at_ns
            || request.requested_at_ns > self.snapshot.valid_until_ns
        {
            return Err(EconomicsUnavailable::StaleSource {
                source_id: SourceId::new(self.snapshot.source_id.clone())?,
            });
        }
        let components = self
            .snapshot
            .components
            .iter()
            .map(canonical_component)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(VenueQuoteEstimate {
            authority: source_validity(
                &self.snapshot.source_id,
                &self.snapshot.snapshot_id,
                self.snapshot.source_at_ns,
                self.snapshot.fetched_at_ns,
                self.snapshot.valid_until_ns,
            )?,
            components,
        })
    }
}

fn validate_snapshot(snapshot: &HistoricalEconomicsSnapshot) -> Result<(), EconomicsUnavailable> {
    if snapshot.source_at_ns > snapshot.fetched_at_ns
        || snapshot.fetched_at_ns > snapshot.valid_until_ns
        || snapshot.edge_basis.valid_until_ns < snapshot.source_at_ns
        || decimal(&snapshot.edge_basis.normalized_amount)? <= Decimal::ZERO
    {
        return Err(EconomicsUnavailable::InvalidSourceTimeline {
            source_id: SourceId::new(snapshot.source_id.clone())?,
        });
    }
    SourceId::new(snapshot.source_id.clone())?;
    ExecutionClientId::new(snapshot.execution_client_id.clone())?;
    AccountId::new(snapshot.account_id.clone())?;
    ProductSurfaceId::new(snapshot.product_surface_id.clone())?;
    ReportingPolicyId::new(snapshot.reporting_policy_id.clone())?;
    NativeUnitId::new(snapshot.reporting_unit.clone())?;
    SnapshotId::new(snapshot.snapshot_id.clone())?;
    EdgeBasisPolicyId::new(snapshot.edge_basis.policy_id.clone())?;
    for snapshot_id in &snapshot.edge_basis.source_snapshot_ids {
        SnapshotId::new(snapshot_id.clone())?;
    }
    Ok(())
}

fn canonical_component(
    component: &HistoricalEconomicComponent,
) -> Result<EstimatedEconomicComponent, EconomicsUnavailable> {
    let native_unit = NativeUnitId::new(component.native_unit.clone())?;
    let native_amount = decimal(&component.native_amount)?;
    let point_effect = SignedNativeEffect::currency(native_amount, native_unit.clone())?;
    let class = match component.class {
        HistoricalEconomicClass::Charge => EconomicClass::Charge,
        HistoricalEconomicClass::Credit => EconomicClass::Credit,
    };
    if (class == EconomicClass::Charge) != native_amount.is_sign_negative() {
        return Err(EconomicsUnavailable::EconomicClassSignMismatch);
    }
    let admission_treatment = match component.treatment {
        HistoricalAdmissionTreatment::GuaranteedConditionalOnAction => {
            AdmissionTreatment::GuaranteedConditionalOnAction
        }
        HistoricalAdmissionTreatment::VenueRiskBound => AdmissionTreatment::RiskBound {
            authority: RiskBoundAuthority::VenueMaximum,
        },
        HistoricalAdmissionTreatment::OperatorRiskBound => AdmissionTreatment::RiskBound {
            authority: RiskBoundAuthority::OperatorRiskLimit,
        },
        HistoricalAdmissionTreatment::ForecastOnly => AdmissionTreatment::ForecastOnly,
    };
    let debit_risk_bound = component
        .debit_risk_bound
        .as_deref()
        .map(decimal)
        .transpose()?
        .map(|amount| SignedNativeEffect::currency(amount, native_unit.clone()))
        .transpose()?;
    let normalized = component
        .valuation
        .as_ref()
        .map(|valuation| canonical_valuation(&point_effect, valuation))
        .transpose()?;
    Ok(EstimatedEconomicComponent {
        component_id: EconomicComponentId::new(component.component_id.clone())?,
        class,
        kind: EconomicKind::Execution(ExecutionKind::ProtocolTrading),
        scope: EconomicScope::Order {
            order_id: OrderId::new(component.order_id.clone())?,
        },
        point_effect,
        debit_risk_bound,
        admission_treatment,
        calculation_factors: Vec::new(),
        formula_id: FormulaId::new(component.formula_id.clone())?,
        source: source_validity(
            &component.source_id,
            &component.snapshot_id,
            component.source_at_ns,
            component.fetched_at_ns,
            component.valid_until_ns,
        )?,
        normalized,
    })
}

fn canonical_valuation(
    native_effect: &SignedNativeEffect,
    valuation: &HistoricalValuationEvidence,
) -> Result<ValuationEvidence, EconomicsUnavailable> {
    Ok(ValuationEvidence {
        native_effect: native_effect.clone(),
        normalized_amount: decimal(&valuation.normalized_amount)?,
        reporting_unit: NativeUnitId::new(valuation.reporting_unit.clone())?,
        route_id: valuation
            .route_id
            .as_ref()
            .map(|value| bolt_v2::economics::ValuationRouteId::new(value.clone()))
            .transpose()?,
        source_snapshot_ids: valuation
            .source_snapshot_ids
            .iter()
            .map(|value| SnapshotId::new(value.clone()))
            .collect::<Result<Vec<_>, _>>()?,
        valued_at_ns: valuation.valued_at_ns,
        valid_until_ns: valuation.valid_until_ns,
    })
}

fn source_validity(
    source_id: &str,
    snapshot_id: &str,
    source_at_ns: u64,
    fetched_at_ns: u64,
    valid_until_ns: u64,
) -> Result<SourceValidity, EconomicsUnavailable> {
    if source_at_ns > fetched_at_ns || fetched_at_ns > valid_until_ns {
        return Err(EconomicsUnavailable::InvalidSourceTimeline {
            source_id: SourceId::new(source_id)?,
        });
    }
    Ok(SourceValidity {
        source_id: SourceId::new(source_id)?,
        snapshot_id: SnapshotId::new(snapshot_id)?,
        source_at_ns,
        fetched_at_ns,
        valid_until_ns,
    })
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
            attached_charge: None,
        },
        position: None,
        lifecycle_path: LifecyclePath::PlannedExit,
        reporting_policy_id: ReportingPolicyId::new(intent.reporting_policy_id)?,
        reporting_unit: NativeUnitId::new(intent.reporting_unit)?,
        edge_basis_policy_id: EdgeBasisPolicyId::new(intent.edge_basis_policy_id)?,
        requested_at_ns: intent.requested_at_ns,
        decision_correlation_id: DecisionCorrelationId::new(intent.decision_correlation_id)?,
    })
}
