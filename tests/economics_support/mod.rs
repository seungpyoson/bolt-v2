use std::str::FromStr;

use bolt_v2::economics::{
    AccountId, AdmissionTreatment, DecisionCorrelationId, EconomicClass, EconomicComponentId,
    EconomicKind, EconomicQuote, EconomicQuoteRequest, EconomicScope, EconomicsUnavailable,
    EdgeBasisPolicyId, EstimatedEconomicComponent, ExecutionClientId, ExecutionKind, FormulaId,
    InstrumentId, LifecyclePath, LiquidityRoleAssumption, NativeUnitId, OrderId, OrderSide,
    PlannedFillLeg, ProductSurfaceId, ReportingPolicyId, RiskBoundAuthority, RoutingContext,
    SignedNativeEffect, SnapshotId, SourceId, SourceValidity, validate_and_aggregate_quote,
};
use rust_decimal::Decimal;

pub fn decimal(value: &str) -> Decimal {
    Decimal::from_str(value).expect("fixture decimal must parse")
}

pub fn native_unit(value: &str) -> NativeUnitId {
    NativeUnitId::new(value).expect("fixture native unit must be valid")
}

pub fn canonical_fixture_request() -> EconomicQuoteRequest {
    EconomicQuoteRequest {
        execution_client_id: ExecutionClientId::new("execution-client").unwrap(),
        account_id: AccountId::new("account").unwrap(),
        instrument_id: InstrumentId::new("instrument").unwrap(),
        product_surface_id: ProductSurfaceId::new("surface").unwrap(),
        order_side: OrderSide::Buy,
        liquidity_role: LiquidityRoleAssumption::Taker,
        planned_fill_legs: vec![PlannedFillLeg {
            price: decimal("0.50"),
            quantity: decimal("10"),
        }],
        routing: RoutingContext {
            attached_charge: None,
        },
        position: None,
        lifecycle_path: LifecyclePath::PlannedExit,
        reporting_policy_id: ReportingPolicyId::new("reporting-policy").unwrap(),
        reporting_unit: native_unit("pUSD"),
        edge_basis_policy_id: EdgeBasisPolicyId::new("edge-basis-policy").unwrap(),
        requested_at_ns: 100,
        decision_correlation_id: DecisionCorrelationId::new("decision").unwrap(),
    }
}

pub fn estimated_component(
    component_id: &str,
    point: Decimal,
    treatment: AdmissionTreatment,
    debit_bound: Option<Decimal>,
) -> EstimatedEconomicComponent {
    EstimatedEconomicComponent {
        component_id: EconomicComponentId::new(component_id).unwrap(),
        class: if point.is_sign_negative() {
            EconomicClass::Charge
        } else {
            EconomicClass::Credit
        },
        kind: EconomicKind::Execution(ExecutionKind::ProtocolTrading),
        scope: EconomicScope::Order {
            order_id: OrderId::new("order").unwrap(),
        },
        point_effect: SignedNativeEffect::currency(point, native_unit("pUSD")).unwrap(),
        debit_risk_bound: debit_bound
            .map(|bound| SignedNativeEffect::currency(bound, native_unit("pUSD")).unwrap()),
        admission_treatment: treatment,
        calculation_factors: Vec::new(),
        formula_id: FormulaId::new("fixture-formula").unwrap(),
        source: SourceValidity {
            source_id: SourceId::new("fixture-source").unwrap(),
            snapshot_id: SnapshotId::new(component_id).unwrap(),
            source_at_ns: 90,
            fetched_at_ns: 95,
            valid_until_ns: 110,
        },
        normalized: None,
    }
}

pub fn guaranteed(point: Decimal) -> EstimatedEconomicComponent {
    estimated_component(
        "guaranteed",
        point,
        AdmissionTreatment::GuaranteedConditionalOnAction,
        None,
    )
}

pub fn risk_bound(point: Decimal, bound: Decimal) -> EstimatedEconomicComponent {
    estimated_component(
        "risk-bound",
        point,
        AdmissionTreatment::RiskBound {
            authority: RiskBoundAuthority::VenueMaximum,
        },
        Some(bound),
    )
}

pub fn risk_bound_without_debit_bound() -> EstimatedEconomicComponent {
    estimated_component(
        "risk-bound",
        decimal("-0.25"),
        AdmissionTreatment::RiskBound {
            authority: RiskBoundAuthority::OperatorRiskLimit,
        },
        None,
    )
}

pub fn forecast(point: Decimal) -> EstimatedEconomicComponent {
    estimated_component("forecast", point, AdmissionTreatment::ForecastOnly, None)
}

pub fn quote_fixture(
    components: impl IntoIterator<Item = EstimatedEconomicComponent>,
) -> Result<EconomicQuote, EconomicsUnavailable> {
    validate_and_aggregate_quote(
        &canonical_fixture_request(),
        components.into_iter().collect(),
        &[],
    )
}
