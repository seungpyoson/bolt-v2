use std::sync::Arc;

use bolt_v2::{
    bolt_v3_economics_runtime::{
        AuthoritativeEconomicsInputStore, AuthoritativeEconomicsQuoteDependencies,
        BoltV3EconomicsRuntime, ConfiguredEconomicsAdmissionSource, EconomicsAdmissionIntent,
        EconomicsAdmissionQuoteIntent, EconomicsAdmissionSource,
    },
    economics::{
        EconomicQuoteRequest, EconomicScope, EconomicsUnavailable, EdgeBasisEvidence, SnapshotId,
        VenueEconomicsAdapter, VenueQuoteEstimate,
    },
};

use super::economics_support::{canonical_fixture_request, decimal, estimated_component};

struct FixedVenue(VenueQuoteEstimate);

impl VenueEconomicsAdapter for FixedVenue {
    fn quote(
        &self,
        _request: &EconomicQuoteRequest,
    ) -> Result<VenueQuoteEstimate, EconomicsUnavailable> {
        Ok(self.0.clone())
    }
}

fn intent(request: EconomicQuoteRequest) -> EconomicsAdmissionIntent {
    EconomicsAdmissionIntent {
        edge_basis: EdgeBasisEvidence {
            policy_id: request.edge_basis_policy_id.clone(),
            policy_version: 1,
            normalized_amount: decimal("5"),
            scope: EconomicScope::Decision {
                decision_correlation_id: request.decision_correlation_id.clone(),
            },
            source_snapshot_ids: vec![SnapshotId::new("basis-snapshot").unwrap()],
            valid_until_ns: 110,
        },
        request,
        gross_expected_value: decimal("2"),
        valuations: Vec::new(),
        base_reservation_notional: decimal("5"),
    }
}

#[test]
fn quote_admission_reserves_authoritative_debits_once() {
    let request = canonical_fixture_request();
    let component = estimated_component(
        "charge",
        decimal("-0.25"),
        bolt_v2::economics::AdmissionTreatment::GuaranteedConditionalOnAction,
        None,
    );
    let authority = component.source.clone();
    let runtime = BoltV3EconomicsRuntime::from_offline_adapter(
        Arc::new(FixedVenue(VenueQuoteEstimate {
            authority,
            dependency_sources: Vec::new(),
            components: vec![component],
        })),
        10,
    )
    .unwrap();
    let admission = runtime.quote_admission(intent(request)).unwrap();
    assert_eq!(admission.reservation_notional(), decimal("5.25"));
    assert_eq!(admission.net_edge().core_net_edge(), decimal("1.75"));
    assert!(
        admission
            .source_snapshot_ids()
            .contains(&SnapshotId::new("basis-snapshot").unwrap())
    );
}

#[test]
fn configured_quote_validity_caps_authoritative_source_window() {
    let request = canonical_fixture_request();
    let component = estimated_component(
        "charge",
        decimal("-0.25"),
        bolt_v2::economics::AdmissionTreatment::GuaranteedConditionalOnAction,
        None,
    );
    let authority = component.source.clone();
    let runtime = BoltV3EconomicsRuntime::from_offline_adapter(
        Arc::new(FixedVenue(VenueQuoteEstimate {
            authority,
            dependency_sources: Vec::new(),
            components: vec![component],
        })),
        5,
    )
    .unwrap();

    let admission = runtime.quote_admission(intent(request)).unwrap();

    assert_eq!(admission.quote().valid_until_ns(), 105);
}

#[test]
fn configured_source_quotes_from_exact_authoritative_client_instrument_and_surface() {
    let request = canonical_fixture_request();
    let component = estimated_component(
        "charge",
        decimal("-0.25"),
        bolt_v2::economics::AdmissionTreatment::GuaranteedConditionalOnAction,
        None,
    );
    let authority = component.source.clone();
    let inputs = AuthoritativeEconomicsInputStore::default();
    inputs
        .publish(
            request.execution_client_id.as_str(),
            request.instrument_id.as_str(),
            request.product_surface_id.as_str(),
            AuthoritativeEconomicsQuoteDependencies {
                provider_key: "configured-provider".to_string(),
                adapter: Arc::new(FixedVenue(VenueQuoteEstimate {
                    authority,
                    dependency_sources: Vec::new(),
                    components: vec![component],
                })),
                edge_basis: intent(request.clone()).edge_basis,
                valuations: Vec::new(),
            },
        )
        .unwrap();
    let source = ConfiguredEconomicsAdmissionSource::new("configured-provider", inputs, 5)
        .expect("configured source should build");

    let admission = source
        .quote_admission(EconomicsAdmissionQuoteIntent {
            request,
            gross_expected_value: decimal("2"),
            base_reservation_notional: decimal("5"),
        })
        .expect("exact authoritative dependencies should quote");

    assert_eq!(admission.quote().valid_until_ns(), 105);
    assert_eq!(admission.reservation_notional(), decimal("5.25"));
}

#[test]
fn non_positive_core_net_edge_cannot_create_admission() {
    let request = canonical_fixture_request();
    let component = estimated_component(
        "charge",
        decimal("-0.25"),
        bolt_v2::economics::AdmissionTreatment::GuaranteedConditionalOnAction,
        None,
    );
    let authority = component.source.clone();
    let runtime = BoltV3EconomicsRuntime::from_offline_adapter(
        Arc::new(FixedVenue(VenueQuoteEstimate {
            authority,
            dependency_sources: Vec::new(),
            components: vec![component],
        })),
        10,
    )
    .unwrap();
    let mut admission_intent = intent(request);
    admission_intent.gross_expected_value = decimal("0.25");

    assert_eq!(
        runtime.quote_admission(admission_intent),
        Err(EconomicsUnavailable::NonPositiveNetEdge)
    );
}

#[test]
fn stale_authority_cannot_create_admission() {
    let request = canonical_fixture_request();
    let component = estimated_component(
        "charge",
        decimal("-0.25"),
        bolt_v2::economics::AdmissionTreatment::GuaranteedConditionalOnAction,
        None,
    );
    let mut authority = component.source.clone();
    authority.valid_until_ns = 99;
    let runtime = BoltV3EconomicsRuntime::from_offline_adapter(
        Arc::new(FixedVenue(VenueQuoteEstimate {
            authority,
            dependency_sources: Vec::new(),
            components: vec![component],
        })),
        10,
    )
    .unwrap();
    assert!(matches!(
        runtime.quote_admission(intent(request)),
        Err(EconomicsUnavailable::StaleSource { .. })
    ));
}
