use std::sync::Arc;

use bolt_v2::{
    bolt_v3_economics_runtime::{
        AuthoritativeEconomicsInputStore, AuthoritativeEconomicsQuoteDependencies,
        AuthoritativeEdgeBasis, BoltV3EconomicsRuntime, ConfiguredEconomicsAdmissionSource,
        ConfiguredEconomicsSourcePolicy, EconomicsAdmissionIntent, EconomicsAdmissionPurpose,
        EconomicsAdmissionQuoteIntent, EconomicsAdmissionSource, EconomicsOrderBinding,
    },
    economics::{
        EconomicQuoteRequest, EconomicScope, EconomicsUnavailable, EdgeBasisEvidence, FormulaId,
        NativeUnitId, PointEstimate, ProductSurfaceId, ReservationBasis, ResolvedEdgeBasis,
        SignedNativeEffect, SnapshotId, SourceId, VenueEconomicsAdapter, VenueQuoteEstimate,
    },
};

use super::economics_support::{canonical_fixture_request, decimal, estimated_component};

struct FixedVenue(VenueQuoteEstimate);

impl VenueEconomicsAdapter for FixedVenue {
    fn resolve_edge_basis(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<ResolvedEdgeBasis, EconomicsUnavailable> {
        Ok(ResolvedEdgeBasis {
            source_snapshot_ids: vec![SnapshotId::new("basis-snapshot")?],
            valid_until_ns: request.requested_at_ns + 5,
        })
    }

    fn quote(
        &self,
        _request: &EconomicQuoteRequest,
    ) -> Result<VenueQuoteEstimate, EconomicsUnavailable> {
        Ok(self.0.clone())
    }
}

struct MismatchedBasisVenue(VenueQuoteEstimate);

impl VenueEconomicsAdapter for MismatchedBasisVenue {
    fn resolve_edge_basis(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<ResolvedEdgeBasis, EconomicsUnavailable> {
        Ok(ResolvedEdgeBasis {
            source_snapshot_ids: vec![SnapshotId::new("other-basis-snapshot")?],
            valid_until_ns: request.requested_at_ns + 5,
        })
    }

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
                    resolver_id: FormulaId::new("fixture-resolver").unwrap(),
                    product_metadata_source: SourceId::new("fixture-product-metadata").unwrap(),
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
            quote_max_age_ns: 5,
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
    let authority_refreshed_at_ns = request.requested_at_ns;
    EconomicsAdmissionIntent {
        order_binding: test_order_binding(),
        purpose: EconomicsAdmissionPurpose::TradingEdge,
        edge_basis: EdgeBasisEvidence {
            policy_id: request.edge_basis_policy_id.clone(),
            resolver_id: FormulaId::new("fixture-resolver").unwrap(),
            product_metadata_source: SourceId::new("fixture-product-metadata").unwrap(),
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
        reservation_basis: ReservationBasis::new(decimal("5")).expect("valid basis"),
        authority_refreshed_at_ns,
    }
}

fn runtime(estimate: VenueQuoteEstimate, quote_validity_ns: u64) -> BoltV3EconomicsRuntime {
    BoltV3EconomicsRuntime::try_new(
        Arc::new(FixedVenue(estimate)),
        ConfiguredEconomicsSourcePolicy {
            quote_refresh_ns: quote_validity_ns,
            quote_max_age_ns: quote_validity_ns,
            quote_validity_ns,
            resting_order_refresh_margin_ns: 1,
        },
    )
    .expect("valid test economics policy")
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
    let runtime = runtime(
        VenueQuoteEstimate {
            authority,
            dependency_sources: Vec::new(),
            components: vec![component],
        },
        10,
    );
    let admission = runtime.quote_admission(intent(request)).unwrap();
    assert_eq!(
        admission.full_reservation_liability().amount(),
        decimal("5.25")
    );
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
    let runtime = runtime(
        VenueQuoteEstimate {
            authority,
            dependency_sources: Vec::new(),
            components: vec![component],
        },
        5,
    );

    let admission = runtime.quote_admission(intent(request)).unwrap();

    assert_eq!(admission.quote().valid_until_ns(), 105);
}

#[test]
fn missing_forecast_valuation_degrades_without_blocking_core_admission() {
    let request = canonical_fixture_request();
    let mut component = estimated_component(
        "forecast",
        decimal("0.25"),
        bolt_v2::economics::AdmissionTreatment::ForecastOnly,
        None,
    );
    component.point_estimate = PointEstimate::NonZero(
        SignedNativeEffect::currency(
            decimal("0.25"),
            NativeUnitId::new("unvalued-incentive-unit").unwrap(),
        )
        .unwrap(),
    );
    let authority = component.source.clone();
    let runtime = runtime(
        VenueQuoteEstimate {
            authority,
            dependency_sources: Vec::new(),
            components: vec![component],
        },
        10,
    );

    let admission = runtime
        .quote_admission(intent(request))
        .expect("supplemental evidence must not block the core seal");

    assert!(!admission.quote().forecast_complete());
    assert_eq!(admission.quote().core_total(), Decimal::ZERO);
}

#[test]
fn missing_required_valuation_still_blocks_admission() {
    let request = canonical_fixture_request();
    let mut component = estimated_component(
        "required",
        decimal("-0.25"),
        bolt_v2::economics::AdmissionTreatment::GuaranteedConditionalOnAction,
        None,
    );
    component.point_estimate = PointEstimate::NonZero(
        SignedNativeEffect::currency(
            decimal("-0.25"),
            NativeUnitId::new("unvalued-required-unit").unwrap(),
        )
        .unwrap(),
    );
    let authority = component.source.clone();
    let runtime = runtime(
        VenueQuoteEstimate {
            authority,
            dependency_sources: Vec::new(),
            components: vec![component],
        },
        10,
    );

    assert!(matches!(
        runtime.quote_admission(intent(request)),
        Err(EconomicsUnavailable::MissingValuation { .. })
            | Err(EconomicsUnavailable::MissingValuationRoute { .. })
    ));
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
                    resolver_id: FormulaId::new("fixture-resolver").unwrap(),
                    product_metadata_source: SourceId::new("fixture-product-metadata").unwrap(),
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
            quote_max_age_ns: 5,
            quote_validity_ns: 5,
            resting_order_refresh_margin_ns: 1,
        },
    )
    .expect("configured source should build");

    let admission = source
        .quote_admission(EconomicsAdmissionQuoteIntent {
            request,
            order_binding: test_order_binding(),
            purpose: EconomicsAdmissionPurpose::TradingEdge,
            gross_expected_value: decimal("2"),
            reservation_basis: ReservationBasis::new(decimal("5")).expect("valid basis"),
        })
        .expect("exact authoritative dependencies should quote");

    assert_eq!(admission.quote().valid_until_ns(), 105);
    assert_eq!(
        admission.full_reservation_liability().amount(),
        decimal("5.25")
    );
    assert_eq!(admission.net_edge().basis().normalized_amount, decimal("5"));
    assert_eq!(
        admission.net_edge().basis().resolver_id.as_str(),
        "fixture-resolver"
    );
    assert_eq!(
        admission
            .net_edge()
            .basis()
            .product_metadata_source
            .as_str(),
        "fixture-product-metadata"
    );
    assert_eq!(
        admission.net_edge().basis().scope,
        EconomicScope::Decision {
            decision_correlation_id: admission.quote().decision_correlation_id().clone(),
        }
    );
}

#[test]
fn configured_source_keeps_planned_edge_basis_distinct_from_reservation_basis() {
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
                    resolver_id: FormulaId::new("fixture-resolver").unwrap(),
                    product_metadata_source: SourceId::new("fixture-product-metadata").unwrap(),
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
            quote_max_age_ns: 5,
            quote_validity_ns: 5,
            resting_order_refresh_margin_ns: 1,
        },
    )
    .unwrap();

    let admission = source
        .quote_admission(EconomicsAdmissionQuoteIntent {
            request,
            order_binding: test_order_binding(),
            purpose: EconomicsAdmissionPurpose::RiskReduction,
            gross_expected_value: decimal("2"),
            reservation_basis: ReservationBasis::new(decimal("5.50")).expect("valid basis"),
        })
        .expect("planned execution value and reservation basis are distinct facts");

    assert_eq!(admission.net_edge().basis().normalized_amount, decimal("5"));
    assert_eq!(admission.planned_fill_notional().amount(), decimal("5"));
    assert_eq!(admission.reservation_basis().amount(), decimal("5.50"));
    assert_eq!(admission.guaranteed_debit().amount(), decimal("0.25"));
    assert_eq!(
        admission.full_reservation_liability().amount(),
        decimal("5.75")
    );
}

#[test]
fn configured_source_rejects_edge_basis_provenance_disagreement() {
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
                adapter: Arc::new(MismatchedBasisVenue(VenueQuoteEstimate {
                    authority: component.source.clone(),
                    dependency_sources: Vec::new(),
                    components: vec![component],
                })),
                edge_basis: AuthoritativeEdgeBasis {
                    resolver_id: FormulaId::new("fixture-resolver").unwrap(),
                    product_metadata_source: SourceId::new("fixture-product-metadata").unwrap(),
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
            quote_max_age_ns: 5,
            quote_validity_ns: 5,
            resting_order_refresh_margin_ns: 1,
        },
    )
    .unwrap();

    let error = source
        .quote_admission(EconomicsAdmissionQuoteIntent {
            request,
            order_binding: test_order_binding(),
            purpose: EconomicsAdmissionPurpose::TradingEdge,
            gross_expected_value: decimal("2"),
            reservation_basis: ReservationBasis::new(decimal("5")).expect("valid basis"),
        })
        .expect_err("basis provenance disagreement must fail closed");

    assert_eq!(error, EconomicsUnavailable::InvalidEdgeBasis);
}

fn assert_configured_source_rejects_stale_dependencies(policy: ConfiguredEconomicsSourcePolicy) {
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
                    resolver_id: FormulaId::new("fixture-resolver").unwrap(),
                    product_metadata_source: SourceId::new("fixture-product-metadata").unwrap(),
                    policy_version: 1,
                    source_snapshot_ids: vec![SnapshotId::new("basis-snapshot").unwrap()],
                    valid_until_ns: request.requested_at_ns + 5,
                },
                valuation_provider: bolt_v2::bolt_v3_economics_runtime::identity_valuation_provider(
                ),
            },
        )
        .unwrap();
    let source =
        ConfiguredEconomicsAdmissionSource::new("configured-provider", inputs, policy).unwrap();

    let error = source
        .quote_admission(EconomicsAdmissionQuoteIntent {
            request,
            order_binding: test_order_binding(),
            purpose: EconomicsAdmissionPurpose::TradingEdge,
            gross_expected_value: decimal("2"),
            reservation_basis: ReservationBasis::new(decimal("5")).expect("valid basis"),
        })
        .expect_err("expired source deadline must fail closed");

    assert!(matches!(error, EconomicsUnavailable::StaleSource { .. }));
}

#[test]
fn configured_source_rejects_dependencies_past_the_refresh_deadline() {
    assert_configured_source_rejects_stale_dependencies(ConfiguredEconomicsSourcePolicy {
        quote_refresh_ns: 5,
        quote_max_age_ns: 10,
        quote_validity_ns: 5,
        resting_order_refresh_margin_ns: 1,
    });
}

#[test]
fn configured_source_rejects_dependencies_past_the_maximum_age() {
    assert_configured_source_rejects_stale_dependencies(ConfiguredEconomicsSourcePolicy {
        quote_refresh_ns: 10,
        quote_max_age_ns: 5,
        quote_validity_ns: 5,
        resting_order_refresh_margin_ns: 1,
    });
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
                    resolver_id: FormulaId::new("fixture-resolver").unwrap(),
                    product_metadata_source: SourceId::new("fixture-product-metadata").unwrap(),
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
            quote_max_age_ns: 10,
            quote_validity_ns: 10,
            resting_order_refresh_margin_ns: 6,
        },
    )
    .unwrap();

    let error = source
        .quote_admission(EconomicsAdmissionQuoteIntent {
            request,
            order_binding: test_order_binding(),
            purpose: EconomicsAdmissionPurpose::TradingEdge,
            gross_expected_value: decimal("2"),
            reservation_basis: ReservationBasis::new(decimal("5")).expect("valid basis"),
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
    let runtime = runtime(
        VenueQuoteEstimate {
            authority,
            dependency_sources: Vec::new(),
            components: vec![component],
        },
        10,
    );
    let mut admission_intent = intent(request);
    admission_intent.gross_expected_value = decimal("0.25");

    assert_eq!(
        runtime.quote_admission(admission_intent),
        Err(EconomicsUnavailable::NonPositiveNetEdge)
    );
}

#[test]
fn risk_reduction_admission_retains_non_positive_edge_and_debit_reservation() {
    let request = canonical_fixture_request();
    let component = estimated_component(
        "charge",
        decimal("-0.25"),
        bolt_v2::economics::AdmissionTreatment::GuaranteedConditionalOnAction,
        None,
    );
    let authority = component.source.clone();
    let runtime = runtime(
        VenueQuoteEstimate {
            authority,
            dependency_sources: Vec::new(),
            components: vec![component],
        },
        10,
    );
    let mut admission_intent = intent(request);
    admission_intent.purpose = EconomicsAdmissionPurpose::RiskReduction;
    admission_intent.gross_expected_value = decimal("0.25");

    let admission = runtime
        .quote_admission(admission_intent)
        .expect("risk reduction must retain a fresh loss-making quote");

    assert_eq!(
        admission.purpose(),
        EconomicsAdmissionPurpose::RiskReduction
    );
    assert_eq!(admission.net_edge().core_net_edge(), decimal("0"));
    assert_eq!(admission.guaranteed_debit().amount(), decimal("0.25"));
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
    let runtime = runtime(
        VenueQuoteEstimate {
            authority,
            dependency_sources: Vec::new(),
            components: vec![component],
        },
        10,
    );
    assert!(matches!(
        runtime.quote_admission(intent(request)),
        Err(EconomicsUnavailable::StaleSource { .. })
    ));
}
