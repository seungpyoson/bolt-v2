use std::sync::Arc;

use bolt_v2::{
    bolt_v3_economics_runtime::{
        AuthoritativeEconomicsInputStore, AuthoritativeEconomicsQuoteDependencies,
        AuthoritativeEdgeBasis, BoltV3EconomicsRuntime, ConfiguredEconomicsAdmissionSource,
        ConfiguredEconomicsSourcePolicy, EconomicsAdmissionIntent, EconomicsAdmissionQuoteIntent,
        EconomicsAdmissionSource, EconomicsOrderBinding,
    },
    economics::{
        EconomicQuoteRequest, EconomicScope, EconomicsUnavailable, EdgeBasisEvidence,
        ProductSurfaceId, SnapshotId, VenueEconomicsAdapter, VenueQuoteEstimate,
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

#[test]
fn configured_source_resolves_the_one_published_surface_for_an_instrument() {
    let request = canonical_fixture_request();
    let component = estimated_component(
        "charge",
        decimal("-0.25"),
        bolt_v2::economics::AdmissionTreatment::GuaranteedConditionalOnAction,
        None,
    );
    let inputs = AuthoritativeEconomicsInputStore::default();
    inputs
        .publish(
            request.execution_client_id.as_str(),
            request.instrument_id.as_str(),
            request.product_surface_id.as_str(),
            AuthoritativeEconomicsQuoteDependencies {
                provider_key: "configured-provider".to_string(),
                refreshed_at_ns: request.requested_at_ns,
                adapter: Arc::new(FixedVenue(VenueQuoteEstimate {
                    authority: component.source.clone(),
                    dependency_sources: Vec::new(),
                    components: vec![component],
                })),
                edge_basis: AuthoritativeEdgeBasis {
                    policy_version: 1,
                    source_snapshot_ids: vec![SnapshotId::new("basis-snapshot").unwrap()],
                    valid_until_ns: request.requested_at_ns + 5,
                },
                valuation_provider: bolt_v2::bolt_v3_economics_runtime::identity_valuation_provider(
                ),
            },
        )
        .unwrap();
    let source = ConfiguredEconomicsAdmissionSource::new(
        "configured-provider",
        inputs,
        ConfiguredEconomicsSourcePolicy {
            quote_refresh_ns: 5,
            quote_validity_ns: 5,
            resting_order_refresh_margin_ns: 1,
        },
    )
    .unwrap();

    let selected = source
        .resolve_product_surface(
            &request.execution_client_id,
            &request.instrument_id,
            &[
                ProductSurfaceId::new("unused-surface").unwrap(),
                request.product_surface_id.clone(),
            ],
        )
        .expect("the published instrument surface must be selected");

    assert_eq!(selected, request.product_surface_id);
}

fn intent(request: EconomicQuoteRequest) -> EconomicsAdmissionIntent {
    EconomicsAdmissionIntent {
        order_binding: test_order_binding(),
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
        valuation_provider: bolt_v2::bolt_v3_economics_runtime::identity_valuation_provider(),
        base_reservation_notional: decimal("5"),
    }
}

fn test_order_binding() -> EconomicsOrderBinding {
    EconomicsOrderBinding::from_sha256(<sha2::Sha256 as sha2::Digest>::digest(
        b"test-order-binding",
    ))
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
                refreshed_at_ns: request.requested_at_ns,
                adapter: Arc::new(FixedVenue(VenueQuoteEstimate {
                    authority,
                    dependency_sources: Vec::new(),
                    components: vec![component],
                })),
                edge_basis: AuthoritativeEdgeBasis {
                    policy_version: 1,
                    source_snapshot_ids: vec![SnapshotId::new("basis-snapshot").unwrap()],
                    valid_until_ns: request.requested_at_ns + 5,
                },
                valuation_provider: bolt_v2::bolt_v3_economics_runtime::identity_valuation_provider(
                ),
            },
        )
        .unwrap();
    let source = ConfiguredEconomicsAdmissionSource::new(
        "configured-provider",
        inputs,
        ConfiguredEconomicsSourcePolicy {
            quote_refresh_ns: 5,
            quote_validity_ns: 5,
            resting_order_refresh_margin_ns: 1,
        },
    )
    .expect("configured source should build");

    let admission = source
        .quote_admission(EconomicsAdmissionQuoteIntent {
            request,
            order_binding: test_order_binding(),
            gross_expected_value: decimal("2"),
            base_reservation_notional: decimal("5"),
        })
        .expect("exact authoritative dependencies should quote");

    assert_eq!(admission.quote().valid_until_ns(), 105);
    assert_eq!(admission.reservation_notional(), decimal("5.25"));
    assert_eq!(admission.net_edge().basis().normalized_amount, decimal("5"));
    assert_eq!(
        admission.net_edge().basis().scope,
        EconomicScope::Decision {
            decision_correlation_id: admission.quote().decision_correlation_id().clone(),
        }
    );
}

#[test]
fn configured_source_rejects_dependencies_past_the_refresh_deadline() {
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
                refreshed_at_ns: request.requested_at_ns - 6,
                adapter: Arc::new(FixedVenue(VenueQuoteEstimate {
                    authority,
                    dependency_sources: Vec::new(),
                    components: vec![component],
                })),
                edge_basis: AuthoritativeEdgeBasis {
                    policy_version: 1,
                    source_snapshot_ids: vec![SnapshotId::new("basis-snapshot").unwrap()],
                    valid_until_ns: request.requested_at_ns + 5,
                },
                valuation_provider: bolt_v2::bolt_v3_economics_runtime::identity_valuation_provider(
                ),
            },
        )
        .unwrap();
    let source = ConfiguredEconomicsAdmissionSource::new(
        "configured-provider",
        inputs,
        ConfiguredEconomicsSourcePolicy {
            quote_refresh_ns: 5,
            quote_validity_ns: 5,
            resting_order_refresh_margin_ns: 1,
        },
    )
    .unwrap();

    let error = source
        .quote_admission(EconomicsAdmissionQuoteIntent {
            request,
            order_binding: test_order_binding(),
            gross_expected_value: decimal("2"),
            base_reservation_notional: decimal("5"),
        })
        .expect_err("expired refresh deadline must fail closed");

    assert!(matches!(error, EconomicsUnavailable::StaleSource { .. }));
}

#[test]
fn configured_source_rejects_maker_quote_shorter_than_resting_margin() {
    let mut request = canonical_fixture_request();
    request.liquidity_role = bolt_v2::economics::LiquidityRoleAssumption::GuaranteedMaker;
    let mut component = estimated_component(
        "charge",
        decimal("-0.25"),
        bolt_v2::economics::AdmissionTreatment::GuaranteedConditionalOnAction,
        None,
    );
    component.source.valid_until_ns = request.requested_at_ns + 5;
    let authority = component.source.clone();
    let inputs = AuthoritativeEconomicsInputStore::default();
    inputs
        .publish(
            request.execution_client_id.as_str(),
            request.instrument_id.as_str(),
            request.product_surface_id.as_str(),
            AuthoritativeEconomicsQuoteDependencies {
                provider_key: "configured-provider".to_string(),
                refreshed_at_ns: request.requested_at_ns,
                adapter: Arc::new(FixedVenue(VenueQuoteEstimate {
                    authority,
                    dependency_sources: Vec::new(),
                    components: vec![component],
                })),
                edge_basis: AuthoritativeEdgeBasis {
                    policy_version: 1,
                    source_snapshot_ids: vec![SnapshotId::new("basis-snapshot").unwrap()],
                    valid_until_ns: request.requested_at_ns + 5,
                },
                valuation_provider: bolt_v2::bolt_v3_economics_runtime::identity_valuation_provider(
                ),
            },
        )
        .unwrap();
    let source = ConfiguredEconomicsAdmissionSource::new(
        "configured-provider",
        inputs,
        ConfiguredEconomicsSourcePolicy {
            quote_refresh_ns: 10,
            quote_validity_ns: 10,
            resting_order_refresh_margin_ns: 6,
        },
    )
    .unwrap();

    let error = source
        .quote_admission(EconomicsAdmissionQuoteIntent {
            request,
            order_binding: test_order_binding(),
            gross_expected_value: decimal("2"),
            base_reservation_notional: decimal("5"),
        })
        .expect_err("maker quote shorter than resting margin must fail closed");

    assert!(matches!(error, EconomicsUnavailable::StaleSource { .. }));
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
