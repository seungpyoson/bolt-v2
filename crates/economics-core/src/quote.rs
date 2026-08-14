use std::collections::{BTreeSet, HashSet};

use rust_decimal::Decimal;

use crate::{
    AccountId, AdmissionTreatment, CurrencyId, DecisionCorrelationId, EconomicClass,
    EconomicComponentId, EconomicScope, EconomicsCapabilityHealth, EconomicsError,
    EconomicsInstrumentId, EdgeBasisPolicyId, EstimatedEffect, ExecutionClientId, LifecyclePath,
    LiquidityRole, OrderSide, PlannedFillLeg, PointEstimate, PositionContext, ProductSurfaceId,
    ReportingPolicyId, RoutingContext, SnapshotId, SourceValidity, ValuationEvidence,
    ValuationRoute, value_with_routes,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EconomicsQuoteRequest {
    pub execution_client_id: ExecutionClientId,
    pub account_id: AccountId,
    pub instrument_id: EconomicsInstrumentId,
    pub product_surface_id: ProductSurfaceId,
    pub order_side: OrderSide,
    pub liquidity_role: LiquidityRole,
    pub planned_fill_legs: Vec<PlannedFillLeg>,
    pub routing: RoutingContext,
    pub position: Option<PositionContext>,
    pub lifecycle_path: LifecyclePath,
    pub reporting_policy_id: ReportingPolicyId,
    pub reporting_currency: CurrencyId,
    pub edge_basis_policy_id: EdgeBasisPolicyId,
    pub requested_at_ns: u64,
    pub decision_correlation_id: DecisionCorrelationId,
}

impl EconomicsQuoteRequest {
    pub fn validate(&self) -> Result<(), EconomicsError> {
        PlannedFillNotional::from_legs(&self.planned_fill_legs)?;
        if let Some(position) = &self.position {
            if position.quantity <= Decimal::ZERO {
                return Err(EconomicsError::NonPositiveValue {
                    field: "position_quantity",
                });
            }
            if position.holding_horizon_ns == 0 {
                return Err(EconomicsError::MissingHoldingHorizon);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedFillNotional(Decimal);

impl PlannedFillNotional {
    pub fn from_legs(legs: &[PlannedFillLeg]) -> Result<Self, EconomicsError> {
        if legs.is_empty() {
            return Err(EconomicsError::InvalidPlannedFill);
        }
        let amount = legs.iter().try_fold(Decimal::ZERO, |total, leg| {
            if leg.price <= Decimal::ZERO || leg.quantity <= Decimal::ZERO {
                return None;
            }
            total.checked_add(leg.price.checked_mul(leg.quantity)?)
        });
        let amount = amount.ok_or(EconomicsError::InvalidPlannedFill)?;
        if amount <= Decimal::ZERO {
            return Err(EconomicsError::InvalidPlannedFill);
        }
        Ok(Self(amount))
    }

    pub const fn amount(self) -> Decimal {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueQuoteEstimate {
    pub authority: SourceValidity,
    pub dependency_sources: Vec<SourceValidity>,
    pub components: Vec<EstimatedEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluatedEconomicsComponent {
    component: EstimatedEffect,
    point_valuation: Option<ValuationEvidence>,
    debit_risk_bound_valuation: Option<ValuationEvidence>,
}

impl EvaluatedEconomicsComponent {
    pub fn component(&self) -> &EstimatedEffect {
        &self.component
    }

    pub fn point_valuation(&self) -> Option<&ValuationEvidence> {
        self.point_valuation.as_ref()
    }

    pub fn debit_risk_bound_valuation(&self) -> Option<&ValuationEvidence> {
        self.debit_risk_bound_valuation.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EconomicsQuote {
    decision_correlation_id: DecisionCorrelationId,
    requested_at_ns: u64,
    edge_basis_policy_id: EdgeBasisPolicyId,
    components: Vec<EvaluatedEconomicsComponent>,
    core_total: Decimal,
    forecast_total: Decimal,
    forecast_complete: bool,
    missing_forecast_component_ids: Vec<EconomicComponentId>,
    source_snapshot_ids: Vec<SnapshotId>,
    reporting_currency: CurrencyId,
    valid_until_ns: u64,
    forecast_valid_until_ns: Option<u64>,
}

impl EconomicsQuote {
    pub fn decision_correlation_id(&self) -> &DecisionCorrelationId {
        &self.decision_correlation_id
    }

    pub fn requested_at_ns(&self) -> u64 {
        self.requested_at_ns
    }

    pub fn edge_basis_policy_id(&self) -> &EdgeBasisPolicyId {
        &self.edge_basis_policy_id
    }

    pub fn components(&self) -> &[EvaluatedEconomicsComponent] {
        &self.components
    }

    pub fn core_total(&self) -> Decimal {
        self.core_total
    }

    pub fn forecast_total(&self) -> Decimal {
        self.forecast_total
    }

    pub fn forecast_complete(&self) -> bool {
        self.forecast_complete
    }

    pub fn missing_forecast_component_ids(&self) -> &[EconomicComponentId] {
        &self.missing_forecast_component_ids
    }

    pub fn source_snapshot_ids(&self) -> &[SnapshotId] {
        &self.source_snapshot_ids
    }

    pub fn reporting_currency(&self) -> &CurrencyId {
        &self.reporting_currency
    }

    pub fn valid_until_ns(&self) -> u64 {
        self.valid_until_ns
    }

    pub fn forecast_valid_until_ns(&self) -> Option<u64> {
        self.forecast_valid_until_ns
    }

    pub fn capability_health(&self) -> EconomicsCapabilityHealth {
        EconomicsCapabilityHealth::quote_only(self.valid_until_ns, self.forecast_valid_until_ns)
    }

    pub fn cap_valid_until_ns(&mut self, valid_until_ns: u64) -> Result<(), EconomicsError> {
        if valid_until_ns < self.requested_at_ns {
            return Err(EconomicsError::RequiredCapabilityStale { valid_until_ns });
        }
        self.valid_until_ns = self.valid_until_ns.min(valid_until_ns);
        self.forecast_valid_until_ns = self
            .forecast_valid_until_ns
            .map(|deadline| deadline.min(valid_until_ns));
        Ok(())
    }
}

pub fn validate_and_aggregate_quote(
    request: &EconomicsQuoteRequest,
    estimate: VenueQuoteEstimate,
    routes: &[ValuationRoute],
) -> Result<EconomicsQuote, EconomicsError> {
    request.validate()?;
    validate_component_source(request.requested_at_ns, &estimate.authority)?;
    for source in &estimate.dependency_sources {
        validate_component_source(request.requested_at_ns, source)?;
    }

    let mut component_ids = HashSet::new();
    let mut components = Vec::with_capacity(estimate.components.len());
    let mut core_total = Decimal::ZERO;
    let mut forecast_total = Decimal::ZERO;
    let mut forecast_complete = true;
    let mut missing_forecast_component_ids = Vec::new();
    let mut source_snapshot_ids = BTreeSet::from([estimate.authority.snapshot_id.clone()]);
    source_snapshot_ids.extend(
        estimate
            .dependency_sources
            .iter()
            .map(|source| source.snapshot_id.clone()),
    );
    let mut valid_until_ns = estimate
        .dependency_sources
        .iter()
        .fold(estimate.authority.valid_until_ns, |deadline, source| {
            deadline.min(source.valid_until_ns)
        });
    let mut forecast_valid_until_ns = Some(valid_until_ns);

    for component in estimate.components {
        if !component_ids.insert(component.component_id.clone()) {
            return Err(EconomicsError::DuplicateComponent {
                component_id: component.component_id,
            });
        }
        validate_component_scope(request, &component)?;
        validate_factors(&component)?;
        validate_component_sign(&component)?;
        source_snapshot_ids.insert(component.source.snapshot_id.clone());

        let source_is_fresh = validate_component_source(request.requested_at_ns, &component.source);
        if let Err(error) = source_is_fresh {
            if component.admission_treatment == AdmissionTreatment::ForecastOnly {
                forecast_complete = false;
                forecast_valid_until_ns = None;
                missing_forecast_component_ids.push(component.component_id.clone());
                components.push(EvaluatedEconomicsComponent {
                    component,
                    point_valuation: None,
                    debit_risk_bound_valuation: None,
                });
                continue;
            }
            return Err(error);
        }

        let point_valuation = match &component.point_estimate {
            PointEstimate::NonZero(effect) => match value_with_routes(
                effect,
                &request.reporting_currency,
                routes,
                request.requested_at_ns,
            ) {
                Ok(valuation) => Some(valuation),
                Err(
                    EconomicsError::MissingValuation { .. } | EconomicsError::StaleValuation { .. },
                ) if component.admission_treatment == AdmissionTreatment::ForecastOnly => {
                    forecast_complete = false;
                    forecast_valid_until_ns = None;
                    missing_forecast_component_ids.push(component.component_id.clone());
                    components.push(EvaluatedEconomicsComponent {
                        component,
                        point_valuation: None,
                        debit_risk_bound_valuation: None,
                    });
                    continue;
                }
                Err(error) => return Err(error),
            },
            PointEstimate::ProvenZero { .. } => None,
        };

        if let Some(point_valuation) = &point_valuation {
            source_snapshot_ids.extend(point_valuation.source_snapshot_ids.iter().cloned());
            forecast_total = forecast_total
                .checked_add(point_valuation.normalized_amount)
                .ok_or(EconomicsError::ArithmeticOverflow)?;
        }

        let debit_risk_bound_valuation = match component.admission_treatment {
            AdmissionTreatment::GuaranteedConditionalOnAction => {
                if let Some(deadline) = point_valuation
                    .as_ref()
                    .and_then(|valuation| valuation.valid_until_ns)
                {
                    valid_until_ns = valid_until_ns.min(deadline);
                }
                let normalized = guaranteed_point_amount(&component, point_valuation.as_ref())?;
                core_total = core_total
                    .checked_add(normalized)
                    .ok_or(EconomicsError::ArithmeticOverflow)?;
                valid_until_ns = valid_until_ns.min(component.source.valid_until_ns);
                None
            }
            AdmissionTreatment::RiskBound { .. } => {
                if let Some(deadline) = point_valuation
                    .as_ref()
                    .and_then(|valuation| valuation.valid_until_ns)
                {
                    valid_until_ns = valid_until_ns.min(deadline);
                }
                let bound = component.debit_risk_bound.as_ref().ok_or_else(|| {
                    EconomicsError::MissingDebitRiskBound {
                        component_id: component.component_id.clone(),
                    }
                })?;
                if bound.amount() >= Decimal::ZERO {
                    return Err(EconomicsError::InvalidDebitRiskBound {
                        component_id: component.component_id,
                    });
                }
                let bound_valuation = value_with_routes(
                    bound,
                    &request.reporting_currency,
                    routes,
                    request.requested_at_ns,
                )?;
                source_snapshot_ids.extend(bound_valuation.source_snapshot_ids.iter().cloned());
                if bound_valuation.normalized_amount >= Decimal::ZERO {
                    return Err(EconomicsError::InvalidDebitRiskBound {
                        component_id: component.component_id,
                    });
                }
                if point_valuation.as_ref().is_some_and(|point| {
                    bound_valuation.normalized_amount > point.normalized_amount
                }) {
                    return Err(EconomicsError::InvalidDebitRiskBound {
                        component_id: component.component_id,
                    });
                }
                core_total = core_total
                    .checked_add(bound_valuation.normalized_amount)
                    .ok_or(EconomicsError::ArithmeticOverflow)?;
                if let Some(deadline) = bound_valuation.valid_until_ns {
                    valid_until_ns = valid_until_ns.min(deadline);
                }
                valid_until_ns = valid_until_ns.min(component.source.valid_until_ns);
                Some(bound_valuation)
            }
            AdmissionTreatment::ForecastOnly => {
                if let Some(deadline) = forecast_valid_until_ns.as_mut() {
                    *deadline = (*deadline).min(component.source.valid_until_ns);
                    if let Some(valuation_deadline) = point_valuation
                        .as_ref()
                        .and_then(|valuation| valuation.valid_until_ns)
                    {
                        *deadline = (*deadline).min(valuation_deadline);
                    }
                }
                None
            }
        };
        components.push(EvaluatedEconomicsComponent {
            component,
            point_valuation,
            debit_risk_bound_valuation,
        });
    }

    if forecast_complete {
        forecast_valid_until_ns =
            forecast_valid_until_ns.map(|deadline| deadline.min(valid_until_ns));
    } else {
        forecast_valid_until_ns = None;
    }

    Ok(EconomicsQuote {
        decision_correlation_id: request.decision_correlation_id.clone(),
        requested_at_ns: request.requested_at_ns,
        edge_basis_policy_id: request.edge_basis_policy_id.clone(),
        components,
        core_total,
        forecast_total,
        forecast_complete,
        missing_forecast_component_ids,
        source_snapshot_ids: source_snapshot_ids.into_iter().collect(),
        reporting_currency: request.reporting_currency.clone(),
        valid_until_ns,
        forecast_valid_until_ns,
    })
}

fn validate_component_scope(
    request: &EconomicsQuoteRequest,
    component: &EstimatedEffect,
) -> Result<(), EconomicsError> {
    let matches_request = match &component.scope {
        EconomicScope::Decision {
            decision_correlation_id,
        } => decision_correlation_id == &request.decision_correlation_id,
        EconomicScope::PositionInterval {
            position_id,
            starts_at_ns,
            ends_at_ns,
        } => request.position.as_ref().is_some_and(|position| {
            position_id == &position.position_id
                && *starts_at_ns == request.requested_at_ns
                && request
                    .requested_at_ns
                    .checked_add(position.holding_horizon_ns)
                    .is_some_and(|expected_end| *ends_at_ns == expected_end)
        }),
        EconomicScope::Action { action_id } => matches!(
            &request.lifecycle_path,
            LifecyclePath::Transfer {
                action_id: request_action_id,
            } if action_id == request_action_id
        ),
    };
    if !matches_request {
        return Err(EconomicsError::EffectScopeMismatch {
            component_id: component.component_id.clone(),
        });
    }
    Ok(())
}

fn validate_component_source(
    requested_at_ns: u64,
    source: &SourceValidity,
) -> Result<(), EconomicsError> {
    if source.source_at_ns > source.fetched_at_ns
        || source.fetched_at_ns > requested_at_ns
        || source.fetched_at_ns > source.valid_until_ns
    {
        return Err(EconomicsError::InvalidSourceTimeline {
            source_id: source.source.clone(),
        });
    }
    if source.valid_until_ns < requested_at_ns {
        return Err(EconomicsError::StaleSource {
            source_id: source.source.clone(),
        });
    }
    Ok(())
}

fn guaranteed_point_amount(
    component: &EstimatedEffect,
    point_valuation: Option<&ValuationEvidence>,
) -> Result<Decimal, EconomicsError> {
    if matches!(component.point_estimate, PointEstimate::ProvenZero { .. }) {
        return Ok(Decimal::ZERO);
    }
    point_valuation
        .map(|valuation| valuation.normalized_amount)
        .ok_or_else(|| EconomicsError::MissingGuaranteedPointValuation {
            component_id: component.component_id.clone(),
        })
}

fn validate_factors(component: &EstimatedEffect) -> Result<(), EconomicsError> {
    let mut factor_ids = HashSet::new();
    if let Some(duplicate) = component
        .calculation_factors
        .iter()
        .find(|factor| !factor_ids.insert(factor.factor_id.clone()))
    {
        return Err(EconomicsError::DuplicateCalculationFactor {
            factor_id: duplicate.factor_id.clone(),
        });
    }
    Ok(())
}

fn validate_component_sign(component: &EstimatedEffect) -> Result<(), EconomicsError> {
    match &component.point_estimate {
        PointEstimate::NonZero(effect)
            if !matches!(
                (component.class, effect.amount().is_sign_negative()),
                (EconomicClass::Charge, true) | (EconomicClass::Credit, false)
            ) =>
        {
            Err(EconomicsError::EconomicClassSignMismatch {
                component_id: component.component_id.clone(),
            })
        }
        PointEstimate::ProvenZero { factor_id }
            if !matches!(
                component.admission_treatment,
                AdmissionTreatment::GuaranteedConditionalOnAction
                    | AdmissionTreatment::RiskBound { .. }
            ) || component.class != EconomicClass::Charge
                || !component
                    .calculation_factors
                    .iter()
                    .any(|factor| factor.factor_id == *factor_id && factor.value.is_zero()) =>
        {
            Err(EconomicsError::EconomicClassSignMismatch {
                component_id: component.component_id.clone(),
            })
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CalculationFactor, EconomicKind, EconomicScope, ExecutionKind, FormulaId, PointEstimate,
        SignedNativeEffect, SnapshotId, SourceIdentity,
    };

    fn id<T>(value: &str, constructor: impl FnOnce(String) -> Result<T, EconomicsError>) -> T {
        constructor(value.to_owned()).expect("fixture identifier should be canonical")
    }

    fn request() -> EconomicsQuoteRequest {
        EconomicsQuoteRequest {
            execution_client_id: id("execution", ExecutionClientId::try_new),
            account_id: id("account", AccountId::try_new),
            instrument_id: id("instrument", EconomicsInstrumentId::try_new),
            product_surface_id: id("surface", ProductSurfaceId::try_new),
            order_side: OrderSide::Buy,
            liquidity_role: LiquidityRole::Taker,
            planned_fill_legs: vec![PlannedFillLeg {
                price: Decimal::new(5, 1),
                quantity: Decimal::new(10, 0),
            }],
            routing: RoutingContext {
                attached_charge: None,
            },
            position: None,
            lifecycle_path: LifecyclePath::PlannedExit,
            reporting_policy_id: id("reporting", ReportingPolicyId::try_new),
            reporting_currency: id("USD", CurrencyId::try_new),
            edge_basis_policy_id: id("basis", EdgeBasisPolicyId::try_new),
            requested_at_ns: 1_000,
            decision_correlation_id: id("decision", DecisionCorrelationId::try_new),
        }
    }

    fn source(valid_until_ns: u64) -> SourceValidity {
        SourceValidity {
            source: id("schedule", SourceIdentity::try_new),
            snapshot_id: id("schedule-1", SnapshotId::try_new),
            source_at_ns: 900,
            fetched_at_ns: 950,
            valid_until_ns,
        }
    }

    fn component(
        component_id: &str,
        amount: Decimal,
        treatment: AdmissionTreatment,
    ) -> EstimatedEffect {
        let class = if amount.is_sign_negative() {
            EconomicClass::Charge
        } else {
            EconomicClass::Credit
        };
        EstimatedEffect {
            component_id: id(component_id, EconomicComponentId::try_new),
            class,
            kind: EconomicKind::Execution(ExecutionKind::ProtocolTrading),
            scope: EconomicScope::Decision {
                decision_correlation_id: id("decision", DecisionCorrelationId::try_new),
            },
            point_estimate: PointEstimate::NonZero(
                SignedNativeEffect::currency(amount, id("USD", CurrencyId::try_new))
                    .expect("non-zero effect should construct"),
            ),
            debit_risk_bound: None,
            admission_treatment: treatment,
            calculation_factors: Vec::new(),
            formula_id: id("formula", FormulaId::try_new),
            source: source(1_100),
        }
    }

    #[test]
    fn aggregate_uses_guaranteed_effects_and_bounds_but_not_forecast_credits() {
        let mut bounded = component(
            "funding",
            Decimal::new(2, 0),
            AdmissionTreatment::RiskBound {
                authority: crate::RiskBoundAuthority::VenueRateCapWithPriceStress,
            },
        );
        bounded.debit_risk_bound = Some(
            SignedNativeEffect::currency(Decimal::new(-3, 0), id("USD", CurrencyId::try_new))
                .expect("negative bound should construct"),
        );
        let quote = validate_and_aggregate_quote(
            &request(),
            VenueQuoteEstimate {
                authority: source(1_100),
                dependency_sources: Vec::new(),
                components: vec![
                    component(
                        "fee",
                        Decimal::new(-2, 0),
                        AdmissionTreatment::GuaranteedConditionalOnAction,
                    ),
                    bounded,
                    component(
                        "reward",
                        Decimal::new(50, 0),
                        AdmissionTreatment::ForecastOnly,
                    ),
                ],
            },
            &[],
        )
        .expect("complete quote should aggregate");

        assert_eq!(quote.core_total(), Decimal::new(-5, 0));
        assert_eq!(quote.forecast_total(), Decimal::new(50, 0));
        let [fee, funding, reward] = quote.components() else {
            panic!("quote must retain all three component evaluations");
        };
        assert_eq!(fee.component().component_id.as_str(), "fee");
        assert_eq!(
            fee.point_valuation()
                .expect("guaranteed fee must retain its point valuation")
                .normalized_amount,
            Decimal::new(-2, 0)
        );
        assert_eq!(funding.component().component_id.as_str(), "funding");
        assert_eq!(
            funding
                .debit_risk_bound_valuation()
                .expect("bounded funding must retain its debit valuation")
                .normalized_amount,
            Decimal::new(-3, 0)
        );
        assert_eq!(reward.component().component_id.as_str(), "reward");
        assert!(reward.debit_risk_bound_valuation().is_none());
    }

    #[test]
    fn guaranteed_proven_zero_is_an_auditable_zero_component() {
        let request = request();
        let mut zero = component(
            "asserted-fee-free",
            Decimal::NEGATIVE_ONE,
            AdmissionTreatment::GuaranteedConditionalOnAction,
        );
        let zero_factor_id = id("asserted-zero", FormulaId::try_new);
        zero.point_estimate = PointEstimate::ProvenZero {
            factor_id: zero_factor_id.clone(),
        };
        zero.calculation_factors = vec![CalculationFactor {
            factor_id: zero_factor_id,
            value: Decimal::ZERO,
        }];

        let quote = validate_and_aggregate_quote(
            &request,
            VenueQuoteEstimate {
                authority: source(1_100),
                dependency_sources: Vec::new(),
                components: vec![zero],
            },
            &[],
        )
        .expect("a proven-zero guaranteed component should aggregate");

        assert_eq!(quote.core_total(), Decimal::ZERO);
        assert_eq!(quote.components().len(), 1);
    }

    #[test]
    fn risk_bound_must_cover_the_point_debit() {
        let mut under_reserved = component(
            "funding",
            Decimal::new(-10, 0),
            AdmissionTreatment::RiskBound {
                authority: crate::RiskBoundAuthority::VenueRateCapWithPriceStress,
            },
        );
        under_reserved.debit_risk_bound = Some(
            SignedNativeEffect::currency(Decimal::NEGATIVE_ONE, id("USD", CurrencyId::try_new))
                .expect("negative bound should construct"),
        );

        assert!(matches!(
            validate_and_aggregate_quote(
                &request(),
                VenueQuoteEstimate {
                    authority: source(1_100),
                    dependency_sources: Vec::new(),
                    components: vec![under_reserved],
                },
                &[],
            ),
            Err(EconomicsError::InvalidDebitRiskBound { .. })
        ));
    }

    #[test]
    fn component_scope_must_belong_to_the_quote_request() {
        let mut foreign_decision = component(
            "foreign-decision",
            Decimal::new(-1, 0),
            AdmissionTreatment::GuaranteedConditionalOnAction,
        );
        foreign_decision.scope = EconomicScope::Decision {
            decision_correlation_id: id("other-decision", DecisionCorrelationId::try_new),
        };
        assert!(matches!(
            validate_and_aggregate_quote(
                &request(),
                VenueQuoteEstimate {
                    authority: source(1_100),
                    dependency_sources: Vec::new(),
                    components: vec![foreign_decision],
                },
                &[],
            ),
            Err(EconomicsError::EffectScopeMismatch { .. })
        ));

        let mut position_request = request();
        position_request.position = Some(PositionContext {
            position_id: id("position", crate::PositionId::try_new),
            side: crate::PositionSide::Long,
            quantity: Decimal::ONE,
            holding_horizon_ns: 100,
        });
        let mut foreign_position = component(
            "foreign-position",
            Decimal::new(-1, 0),
            AdmissionTreatment::GuaranteedConditionalOnAction,
        );
        foreign_position.scope = EconomicScope::PositionInterval {
            position_id: id("other-position", crate::PositionId::try_new),
            starts_at_ns: 1_000,
            ends_at_ns: 1_100,
        };
        assert!(matches!(
            validate_and_aggregate_quote(
                &position_request,
                VenueQuoteEstimate {
                    authority: source(1_100),
                    dependency_sources: Vec::new(),
                    components: vec![foreign_position],
                },
                &[],
            ),
            Err(EconomicsError::EffectScopeMismatch { .. })
        ));

        let mut action_request = request();
        action_request.lifecycle_path = LifecyclePath::Transfer {
            action_id: id("action", crate::ActionId::try_new),
        };
        let mut foreign_action = component(
            "foreign-action",
            Decimal::new(-1, 0),
            AdmissionTreatment::GuaranteedConditionalOnAction,
        );
        foreign_action.scope = EconomicScope::Action {
            action_id: id("other-action", crate::ActionId::try_new),
        };
        assert!(matches!(
            validate_and_aggregate_quote(
                &action_request,
                VenueQuoteEstimate {
                    authority: source(1_100),
                    dependency_sources: Vec::new(),
                    components: vec![foreign_action],
                },
                &[],
            ),
            Err(EconomicsError::EffectScopeMismatch { .. })
        ));
    }

    #[test]
    fn forecast_expiry_is_separate_from_required_quote_health() {
        let mut forecast = component(
            "forecast",
            Decimal::new(5, 0),
            AdmissionTreatment::ForecastOnly,
        );
        forecast.source.valid_until_ns = 1_050;
        let quote = validate_and_aggregate_quote(
            &request(),
            VenueQuoteEstimate {
                authority: source(1_100),
                dependency_sources: Vec::new(),
                components: vec![forecast],
            },
            &[],
        )
        .expect("a fresh supplemental forecast should aggregate");

        assert_eq!(quote.valid_until_ns(), 1_100);
        assert_eq!(quote.forecast_valid_until_ns(), Some(1_050));
        assert!(quote.capability_health().allows_admission(1_075).is_ok());
        assert!(!quote.capability_health().forecast_available(1_075));
    }

    #[test]
    fn missing_required_valuation_and_stale_authority_fail_closed() {
        let foreign = EstimatedEffect {
            point_estimate: PointEstimate::NonZero(
                SignedNativeEffect::currency(Decimal::new(-2, 0), id("TOKEN", CurrencyId::try_new))
                    .expect("non-zero effect should construct"),
            ),
            ..component(
                "fee",
                Decimal::new(-2, 0),
                AdmissionTreatment::GuaranteedConditionalOnAction,
            )
        };
        assert!(matches!(
            validate_and_aggregate_quote(
                &request(),
                VenueQuoteEstimate {
                    authority: source(1_100),
                    dependency_sources: Vec::new(),
                    components: vec![foreign],
                },
                &[],
            ),
            Err(EconomicsError::MissingValuation { .. })
        ));
        assert!(matches!(
            validate_and_aggregate_quote(
                &request(),
                VenueQuoteEstimate {
                    authority: source(999),
                    dependency_sources: Vec::new(),
                    components: Vec::new(),
                },
                &[],
            ),
            Err(EconomicsError::StaleSource { .. })
        ));
    }

    #[test]
    fn position_requires_positive_quantity_and_horizon() {
        let mut request = request();
        request.position = Some(PositionContext {
            position_id: id("position", crate::PositionId::try_new),
            side: crate::PositionSide::Long,
            quantity: Decimal::ONE,
            holding_horizon_ns: 0,
        });
        assert_eq!(
            request.validate(),
            Err(EconomicsError::MissingHoldingHorizon)
        );

        request.position.as_mut().unwrap().holding_horizon_ns = 100;
        request.position.as_mut().unwrap().quantity = Decimal::ZERO;
        assert_eq!(
            request.validate(),
            Err(EconomicsError::NonPositiveValue {
                field: "position_quantity"
            })
        );
    }

    #[test]
    fn guaranteed_component_requires_a_point_valuation_at_the_use_site() {
        let component = component(
            "fee",
            Decimal::NEGATIVE_ONE,
            AdmissionTreatment::GuaranteedConditionalOnAction,
        );
        assert_eq!(
            guaranteed_point_amount(&component, None),
            Err(EconomicsError::MissingGuaranteedPointValuation {
                component_id: id("fee", EconomicComponentId::try_new)
            })
        );
    }
}
