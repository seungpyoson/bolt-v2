use bolt_economics_core::{
    AccountId, AdmissionTreatment, CurrencyId, DecisionCorrelationId, EconomicClass,
    EconomicComponentId, EconomicKind, EconomicScope, EconomicsError, EconomicsInstrumentId,
    EconomicsQuoteRequest, EdgeBasisAmount, EdgeBasisEvidence, EdgeBasisPolicyId, EstimatedEffect,
    ExecutionClientId, ExecutionKind, FormulaId, LifecyclePath, LiquidityRole, OrderSide,
    PlannedFillLeg, PlannedFillNotional, PointEstimate, ProductSurfaceId, ReportingPolicyId,
    RoutingContext, SignedNativeEffect, SnapshotId, SourceIdentity, SourceValidity,
    VenueEconomicsAdapter, VenueEconomicsUnavailable, VenueEdgeBasisEstimate, VenueQuoteEstimate,
    fold_net_edge, validate_and_aggregate_quote,
};
use rust_decimal::Decimal;

fn id<T>(value: &str, constructor: impl FnOnce(String) -> Result<T, EconomicsError>) -> T {
    constructor(value.to_string()).expect("synthetic identifier should be canonical")
}

struct SyntheticSubstrateIntent {
    execution_client_id: String,
    instrument_id: String,
    requested_at_ns: u64,
}

impl SyntheticSubstrateIntent {
    fn into_economics_request(self) -> EconomicsQuoteRequest {
        EconomicsQuoteRequest {
            execution_client_id: id(&self.execution_client_id, ExecutionClientId::try_new),
            account_id: id("synthetic-account", AccountId::try_new),
            instrument_id: id(&self.instrument_id, EconomicsInstrumentId::try_new),
            product_surface_id: id("synthetic-surface", ProductSurfaceId::try_new),
            order_side: OrderSide::Buy,
            liquidity_role: LiquidityRole::Taker,
            planned_fill_legs: vec![PlannedFillLeg {
                price: Decimal::ONE,
                quantity: Decimal::from(10),
            }],
            routing: RoutingContext {
                attached_charge: None,
            },
            position: None,
            lifecycle_path: LifecyclePath::PlannedExit,
            reporting_policy_id: id("synthetic-reporting", ReportingPolicyId::try_new),
            reporting_currency: id("sUSD", CurrencyId::try_new),
            edge_basis_policy_id: id("synthetic-basis", EdgeBasisPolicyId::try_new),
            requested_at_ns: self.requested_at_ns,
            decision_correlation_id: id("synthetic-decision", DecisionCorrelationId::try_new),
        }
    }
}

struct SyntheticVenue;

impl VenueEconomicsAdapter for SyntheticVenue {
    fn provider_key(&self) -> &str {
        "SYNTHETIC"
    }

    fn resolve_edge_basis(
        &self,
        request: &EconomicsQuoteRequest,
        planned_fill_notional: PlannedFillNotional,
    ) -> Result<VenueEdgeBasisEstimate, VenueEconomicsUnavailable> {
        Ok(VenueEdgeBasisEstimate {
            resolver_id: id("synthetic-resolver", FormulaId::try_new),
            product_metadata_source: id("synthetic-product", SourceIdentity::try_new),
            policy_version: 1,
            normalized_amount: EdgeBasisAmount::try_new(planned_fill_notional.amount())?,
            source_snapshot_ids: vec![id("synthetic-product-1", SnapshotId::try_new)],
            valid_until_ns: request.requested_at_ns + 100,
        })
    }

    fn quote(
        &self,
        request: &EconomicsQuoteRequest,
    ) -> Result<VenueQuoteEstimate, VenueEconomicsUnavailable> {
        let source = SourceValidity {
            source: id("synthetic-schedule", SourceIdentity::try_new),
            snapshot_id: id("synthetic-schedule-1", SnapshotId::try_new),
            source_at_ns: request.requested_at_ns - 2,
            fetched_at_ns: request.requested_at_ns - 1,
            valid_until_ns: request.requested_at_ns + 100,
        };
        Ok(VenueQuoteEstimate {
            authority: source.clone(),
            dependency_sources: Vec::new(),
            components: vec![EstimatedEffect {
                component_id: id("synthetic-fee", EconomicComponentId::try_new),
                class: EconomicClass::Charge,
                kind: EconomicKind::Execution(ExecutionKind::ProtocolTrading),
                scope: EconomicScope::Decision {
                    decision_correlation_id: request.decision_correlation_id.clone(),
                },
                point_estimate: PointEstimate::NonZero(SignedNativeEffect::currency(
                    Decimal::NEGATIVE_ONE,
                    request.reporting_currency.clone(),
                )?),
                debit_risk_bound: None,
                admission_treatment: AdmissionTreatment::GuaranteedConditionalOnAction,
                calculation_factors: Vec::new(),
                formula_id: id("synthetic-formula", FormulaId::try_new),
                source,
            }],
        })
    }
}

#[test]
fn new_venue_and_non_nautilus_substrate_use_only_the_neutral_contract() {
    let request = SyntheticSubstrateIntent {
        execution_client_id: "synthetic-execution".to_string(),
        instrument_id: "SYNTH-USD.SYNTHETIC".to_string(),
        requested_at_ns: 1_000,
    }
    .into_economics_request();
    let venue = SyntheticVenue;
    let notional = PlannedFillNotional::from_legs(&request.planned_fill_legs)
        .expect("synthetic planned fill should be valid");
    let estimate = venue.quote(&request).expect("synthetic venue should quote");
    let quote = validate_and_aggregate_quote(&request, estimate, &[])
        .expect("neutral core should aggregate the synthetic quote");
    let basis = venue
        .resolve_edge_basis(&request, notional)
        .expect("synthetic venue should resolve its edge basis");
    let edge = fold_net_edge(
        Decimal::from(2),
        &quote,
        EdgeBasisEvidence {
            policy_id: request.edge_basis_policy_id.clone(),
            resolver_id: basis.resolver_id,
            product_metadata_source: basis.product_metadata_source,
            policy_version: basis.policy_version,
            normalized_amount: basis.normalized_amount,
            scope: EconomicScope::Decision {
                decision_correlation_id: request.decision_correlation_id.clone(),
            },
            source_snapshot_ids: basis.source_snapshot_ids,
            valid_until_ns: basis.valid_until_ns,
        },
    )
    .expect("neutral core should fold the synthetic edge");

    assert_eq!(venue.provider_key(), "SYNTHETIC");
    assert_eq!(quote.core_total(), Decimal::NEGATIVE_ONE);
    assert_eq!(edge.core_net_edge, Decimal::ONE);
    assert_eq!(
        quote.source_snapshot_ids()[0].as_str(),
        "synthetic-schedule-1"
    );
}
