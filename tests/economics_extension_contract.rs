use bolt_v2::economics::{
    AdmissionTreatment, EconomicClass, EconomicComponentId, EconomicKind, EconomicQuoteRequest,
    EconomicScope, EconomicsUnavailable, EstimatedEconomicComponent, ExecutionKind, FormulaId,
    SignedNativeEffect, SnapshotId, SourceId, SourceValidity, VenueEconomicsAdapter,
    VenueQuoteEstimate, validate_and_aggregate_quote,
};

use super::economics_support::{canonical_fixture_request, decimal, native_unit};

struct SyntheticVenue;

impl VenueEconomicsAdapter for SyntheticVenue {
    fn quote(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<VenueQuoteEstimate, EconomicsUnavailable> {
        let authority = SourceValidity {
            source_id: SourceId::new("synthetic-authority").unwrap(),
            snapshot_id: SnapshotId::new("synthetic-snapshot").unwrap(),
            source_at_ns: 90,
            fetched_at_ns: 95,
            valid_until_ns: 110,
        };
        Ok(VenueQuoteEstimate {
            authority: authority.clone(),
            components: vec![EstimatedEconomicComponent {
                component_id: EconomicComponentId::new("synthetic-charge").unwrap(),
                class: EconomicClass::Charge,
                kind: EconomicKind::Execution(ExecutionKind::ProtocolTrading),
                scope: EconomicScope::Decision {
                    decision_correlation_id: request.decision_correlation_id.clone(),
                },
                point_effect: SignedNativeEffect::currency(decimal("-0.25"), native_unit("pUSD"))
                    .unwrap(),
                debit_risk_bound: None,
                admission_treatment: AdmissionTreatment::GuaranteedConditionalOnAction,
                calculation_factors: Vec::new(),
                formula_id: FormulaId::new("synthetic-formula").unwrap(),
                source: authority,
                normalized: None,
            }],
        })
    }
}

struct SyntheticSubstrate;

impl SyntheticSubstrate {
    fn canonical_request(&self) -> EconomicQuoteRequest {
        canonical_fixture_request()
    }
}

#[test]
fn new_venue_and_non_nt_substrate_use_only_shared_contracts() {
    let request = SyntheticSubstrate.canonical_request();
    let estimate = SyntheticVenue.quote(&request).unwrap();
    let quote = validate_and_aggregate_quote(&request, estimate, &[]).unwrap();
    assert_eq!(quote.core_total(), decimal("-0.25"));
}
