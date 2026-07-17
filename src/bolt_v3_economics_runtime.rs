use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use rust_decimal::Decimal;

use crate::economics::{
    EconomicQuote, EconomicQuoteRequest, EconomicsUnavailable, EdgeBasisEvidence, NetEdgeQuote,
    SnapshotId, ValuationEvidence, VenueEconomicsAdapter, fold_net_edge,
    validate_and_aggregate_quote,
};

pub struct EconomicsAdmissionIntent {
    pub request: EconomicQuoteRequest,
    pub gross_expected_value: Decimal,
    pub edge_basis: EdgeBasisEvidence,
    pub valuations: Vec<ValuationEvidence>,
    pub base_reservation_notional: Decimal,
}

pub struct EconomicsAdmissionQuoteIntent {
    pub request: EconomicQuoteRequest,
    pub gross_expected_value: Decimal,
    pub base_reservation_notional: Decimal,
}

pub trait EconomicsAdmissionSource: Send + Sync {
    fn quote_admission(
        &self,
        intent: EconomicsAdmissionQuoteIntent,
    ) -> Result<EconomicsAdmission, EconomicsUnavailable>;
}

#[derive(Clone)]
pub struct AuthoritativeEconomicsQuoteDependencies {
    pub provider_key: String,
    pub adapter: Arc<dyn VenueEconomicsAdapter>,
    pub edge_basis: EdgeBasisEvidence,
    pub valuations: Vec<ValuationEvidence>,
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
}

pub struct ConfiguredEconomicsAdmissionSource {
    provider_key: String,
    inputs: AuthoritativeEconomicsInputStore,
    quote_validity_ns: u64,
}

impl ConfiguredEconomicsAdmissionSource {
    pub fn new(
        provider_key: &str,
        inputs: AuthoritativeEconomicsInputStore,
        quote_validity_ns: u64,
    ) -> Result<Self, EconomicsUnavailable> {
        if provider_key.trim().is_empty() || quote_validity_ns == 0 {
            return Err(EconomicsUnavailable::InvalidQuoteValidityPolicy);
        }
        Ok(Self {
            provider_key: provider_key.to_string(),
            inputs,
            quote_validity_ns,
        })
    }
}

impl EconomicsAdmissionSource for ConfiguredEconomicsAdmissionSource {
    fn quote_admission(
        &self,
        intent: EconomicsAdmissionQuoteIntent,
    ) -> Result<EconomicsAdmission, EconomicsUnavailable> {
        let dependencies = self.inputs.dependencies(&intent.request)?;
        if dependencies.provider_key != self.provider_key {
            return Err(EconomicsUnavailable::AmbiguousQuoteAuthority);
        }
        BoltV3EconomicsRuntime::from_offline_adapter(dependencies.adapter, self.quote_validity_ns)?
            .quote_admission(EconomicsAdmissionIntent {
                request: intent.request,
                gross_expected_value: intent.gross_expected_value,
                edge_basis: dependencies.edge_basis,
                valuations: dependencies.valuations,
                base_reservation_notional: intent.base_reservation_notional,
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EconomicsAdmission {
    request: EconomicQuoteRequest,
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
        let mut quote =
            validate_and_aggregate_quote(&intent.request, estimate, intent.valuations.as_slice())?;
        let configured_valid_until_ns = intent
            .request
            .requested_at_ns
            .checked_add(self.quote_validity_ns)
            .ok_or(EconomicsUnavailable::InvalidPlannedFill)?;
        quote.cap_valid_until_ns(configured_valid_until_ns);
        let net_edge = fold_net_edge(intent.gross_expected_value, &quote, intent.edge_basis)?;
        if net_edge.core_net_edge() <= Decimal::ZERO {
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
            quote,
            net_edge,
            base_reservation_notional: intent.base_reservation_notional,
            reservation_notional,
            source_snapshot_ids,
        })
    }
}

#[cfg(test)]
pub(crate) fn test_economics_admission(base_reservation_notional: Decimal) -> EconomicsAdmission {
    use crate::economics::{
        AccountId, AdmissionTreatment, DecisionCorrelationId, EconomicClass, EconomicComponentId,
        EconomicKind, EconomicQuoteRequest, EconomicScope, EdgeBasisPolicyId,
        EstimatedEconomicComponent, ExecutionClientId, ExecutionKind, FormulaId, InstrumentId,
        LifecyclePath, LiquidityRoleAssumption, NativeUnitId, OrderSide, PlannedFillLeg,
        ProductSurfaceId, ReportingPolicyId, RoutingContext, SignedNativeEffect, SourceId,
        SourceValidity, VenueQuoteEstimate,
    };

    #[derive(Clone)]
    struct TestAdapter(VenueQuoteEstimate);

    impl VenueEconomicsAdapter for TestAdapter {
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
            point_effect: SignedNativeEffect::currency(Decimal::ONE, reporting_unit)
                .expect("valid test effect"),
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
        gross_expected_value: Decimal::ONE,
        edge_basis: EdgeBasisEvidence {
            policy_id: EdgeBasisPolicyId::new("test-edge-policy")
                .expect("valid test edge policy id"),
            policy_version: 1,
            normalized_amount: base_reservation_notional,
            scope: EconomicScope::Decision {
                decision_correlation_id,
            },
            source_snapshot_ids: vec![source.snapshot_id],
            valid_until_ns,
        },
        valuations: Vec::new(),
        base_reservation_notional,
    })
    .expect("test economics admission should quote")
}

#[cfg(test)]
struct TestEconomicsAdmissionSource;

#[cfg(test)]
impl EconomicsAdmissionSource for TestEconomicsAdmissionSource {
    fn quote_admission(
        &self,
        intent: EconomicsAdmissionQuoteIntent,
    ) -> Result<EconomicsAdmission, EconomicsUnavailable> {
        use crate::economics::{
            AdmissionTreatment, EconomicClass, EconomicComponentId, EconomicKind, EconomicScope,
            EstimatedEconomicComponent, ExecutionKind, FormulaId, SignedNativeEffect, SourceId,
            SourceValidity, VenueQuoteEstimate,
        };

        #[derive(Clone)]
        struct TestAdapter(VenueQuoteEstimate);

        impl VenueEconomicsAdapter for TestAdapter {
            fn quote(
                &self,
                _request: &EconomicQuoteRequest,
            ) -> Result<VenueQuoteEstimate, EconomicsUnavailable> {
                Ok(self.0.clone())
            }
        }

        let valid_until_ns = intent.request.requested_at_ns.saturating_add(1);
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
                point_effect: SignedNativeEffect::currency(
                    Decimal::ONE,
                    intent.request.reporting_unit.clone(),
                )?,
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
                policy_version: 1,
                normalized_amount: intent.base_reservation_notional,
                scope: EconomicScope::Decision {
                    decision_correlation_id: intent.request.decision_correlation_id.clone(),
                },
                source_snapshot_ids: vec![source.snapshot_id],
                valid_until_ns,
            },
            request: intent.request,
            gross_expected_value: intent.gross_expected_value,
            valuations: Vec::new(),
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
        },
    )
    .expect("test order routing handle should build")
}
