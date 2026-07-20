use std::collections::HashSet;

use rust_decimal::Decimal;

use super::{
    AdmissionTreatment, EconomicQuote, EconomicQuoteRequest, EconomicsUnavailable,
    EstimatedEconomicComponent, PointEstimate, SignedNativeEffect, ValuationEvidence,
    VenueQuoteEstimate, value_with_route,
};

pub fn validate_and_aggregate_quote(
    request: &EconomicQuoteRequest,
    estimate: VenueQuoteEstimate,
    valuations: &[ValuationEvidence],
) -> Result<EconomicQuote, EconomicsUnavailable> {
    if request.planned_fill_legs.is_empty()
        || request
            .planned_fill_legs
            .iter()
            .any(|leg| leg.price <= Decimal::ZERO || leg.quantity <= Decimal::ZERO)
    {
        return Err(EconomicsUnavailable::InvalidPlannedFill);
    }
    validate_authority_timeline(request, &estimate)?;

    let mut component_ids = HashSet::new();
    let mut accepted = Vec::with_capacity(estimate.components.len());
    let mut normalizations = Vec::new();
    let mut core_total = Decimal::ZERO;
    let mut forecast_total = Decimal::ZERO;
    let mut forecast_complete = true;
    let mut missing_forecast_component_ids = Vec::new();
    let mut required_valid_until_ns = estimate
        .dependency_sources
        .iter()
        .fold(estimate.authority.valid_until_ns, |deadline, source| {
            deadline.min(source.valid_until_ns)
        });

    for mut component in estimate.components {
        if !component_ids.insert(component.component_id.clone()) {
            return Err(EconomicsUnavailable::DuplicateComponent {
                component_id: component.component_id,
            });
        }
        let mut factor_ids = HashSet::new();
        if let Some(duplicate) = component
            .calculation_factors
            .iter()
            .find(|factor| !factor_ids.insert(factor.factor_id.clone()))
        {
            return Err(EconomicsUnavailable::DuplicateCalculationFactor {
                factor_id: duplicate.factor_id.clone(),
            });
        }
        validate_source_timeline(request, &component)?;
        match &component.point_estimate {
            PointEstimate::NonZero(point_effect) => {
                if !matches!(
                    (component.class, point_effect.amount().is_sign_negative()),
                    (super::EconomicClass::Charge, true) | (super::EconomicClass::Credit, false)
                ) {
                    return Err(EconomicsUnavailable::EconomicClassSignMismatch);
                }
            }
            PointEstimate::ProvenZero { factor_id }
                if !matches!(
                    component.admission_treatment,
                    AdmissionTreatment::RiskBound { .. }
                ) || component.class != super::EconomicClass::Charge
                    || component.normalized.is_some()
                    || !component
                        .calculation_factors
                        .iter()
                        .any(|factor| factor.factor_id == *factor_id && factor.value.is_zero()) =>
            {
                return Err(EconomicsUnavailable::InvalidProvenZeroPoint {
                    component_id: component.component_id,
                });
            }
            PointEstimate::ProvenZero { .. } => {}
        }
        if component.source.valid_until_ns < request.requested_at_ns {
            if component.admission_treatment == AdmissionTreatment::ForecastOnly {
                forecast_complete = false;
                missing_forecast_component_ids.push(component.component_id);
                continue;
            }
            return Err(EconomicsUnavailable::StaleSource {
                source_id: component.source.source_id,
            });
        }

        let point_valid_until_ns = if let Some(point_effect) = component.point_estimate.effect() {
            let point_valuation = match resolve_valuation(
                point_effect,
                component.normalized.as_ref(),
                valuations,
                request,
            ) {
                Ok(valuation) => valuation,
                Err(
                    EconomicsUnavailable::MissingValuation { .. }
                    | EconomicsUnavailable::MissingValuationRoute { .. }
                    | EconomicsUnavailable::StaleValuation { .. },
                ) if component.admission_treatment == AdmissionTreatment::ForecastOnly => {
                    forecast_complete = false;
                    missing_forecast_component_ids.push(component.component_id.clone());
                    accepted.push(component);
                    continue;
                }
                Err(error) => return Err(error),
            };
            let valid_until_ns = point_valuation.valid_until_ns;
            forecast_total = forecast_total
                .checked_add(point_valuation.normalized_amount)
                .ok_or(EconomicsUnavailable::InvalidDecimal)?;
            component.normalized = Some(point_valuation.clone());
            normalizations.push(point_valuation);
            valid_until_ns
        } else {
            None
        };

        match component.admission_treatment {
            AdmissionTreatment::GuaranteedConditionalOnAction => {
                core_total = core_total
                    .checked_add(
                        component
                            .normalized
                            .as_ref()
                            .expect("point normalization was assigned")
                            .normalized_amount,
                    )
                    .ok_or(EconomicsUnavailable::InvalidDecimal)?;
                update_valid_until(
                    &mut required_valid_until_ns,
                    component.source.valid_until_ns,
                );
                if let Some(valid_until_ns) = point_valid_until_ns {
                    required_valid_until_ns = required_valid_until_ns.min(valid_until_ns);
                }
            }
            AdmissionTreatment::RiskBound { .. } => {
                let bound = component.debit_risk_bound.as_ref().ok_or_else(|| {
                    EconomicsUnavailable::MissingDebitRiskBound {
                        component_id: component.component_id.clone(),
                    }
                })?;
                if bound.amount() >= Decimal::ZERO {
                    return Err(EconomicsUnavailable::InvalidDebitRiskBound {
                        component_id: component.component_id,
                    });
                }
                let bound_valuation = resolve_valuation(bound, None, valuations, request)?;
                if bound_valuation.normalized_amount >= Decimal::ZERO {
                    return Err(EconomicsUnavailable::InvalidDebitRiskBound {
                        component_id: component.component_id,
                    });
                }
                core_total = core_total
                    .checked_add(bound_valuation.normalized_amount)
                    .ok_or(EconomicsUnavailable::InvalidDecimal)?;
                if let Some(valid_until_ns) = bound_valuation.valid_until_ns {
                    required_valid_until_ns = required_valid_until_ns.min(valid_until_ns);
                }
                normalizations.push(bound_valuation);
                update_valid_until(
                    &mut required_valid_until_ns,
                    component.source.valid_until_ns,
                );
            }
            AdmissionTreatment::ForecastOnly => {}
        }
        accepted.push(component);
    }

    Ok(EconomicQuote {
        decision_correlation_id: request.decision_correlation_id.clone(),
        requested_at_ns: request.requested_at_ns,
        edge_basis_policy_id: request.edge_basis_policy_id.clone(),
        components: accepted,
        normalizations,
        core_total,
        forecast_total,
        forecast_complete,
        missing_forecast_component_ids,
        reporting_unit: request.reporting_unit,
        valid_until_ns: required_valid_until_ns,
    })
}

fn validate_authority_timeline(
    request: &EconomicQuoteRequest,
    estimate: &VenueQuoteEstimate,
) -> Result<(), EconomicsUnavailable> {
    validate_required_source_timeline(request, &estimate.authority)?;
    for source in &estimate.dependency_sources {
        validate_required_source_timeline(request, source)?;
    }
    Ok(())
}

fn validate_required_source_timeline(
    request: &EconomicQuoteRequest,
    source: &super::SourceValidity,
) -> Result<(), EconomicsUnavailable> {
    if source.source_at_ns > source.fetched_at_ns
        || source.fetched_at_ns > request.requested_at_ns
        || source.fetched_at_ns > source.valid_until_ns
    {
        return Err(EconomicsUnavailable::InvalidSourceTimeline {
            source_id: source.source_id.clone(),
        });
    }
    if source.valid_until_ns < request.requested_at_ns {
        return Err(EconomicsUnavailable::StaleSource {
            source_id: source.source_id.clone(),
        });
    }
    Ok(())
}

fn validate_source_timeline(
    request: &EconomicQuoteRequest,
    component: &EstimatedEconomicComponent,
) -> Result<(), EconomicsUnavailable> {
    let source = &component.source;
    if source.source_at_ns > source.fetched_at_ns
        || source.fetched_at_ns > request.requested_at_ns
        || source.fetched_at_ns > source.valid_until_ns
    {
        return Err(EconomicsUnavailable::InvalidSourceTimeline {
            source_id: source.source_id.clone(),
        });
    }
    Ok(())
}

fn resolve_valuation(
    effect: &SignedNativeEffect,
    embedded: Option<&ValuationEvidence>,
    valuations: &[ValuationEvidence],
    request: &EconomicQuoteRequest,
) -> Result<ValuationEvidence, EconomicsUnavailable> {
    if effect.currency_id() == request.reporting_unit {
        let identity = value_with_route(
            effect,
            &request.reporting_unit,
            None,
            request.requested_at_ns,
        )?;
        if embedded.is_some_and(|evidence| evidence != &identity) {
            return Err(EconomicsUnavailable::ValuationEvidenceMismatch);
        }
        return Ok(identity);
    }

    let mut candidates = embedded
        .into_iter()
        .chain(valuations.iter())
        .filter(|evidence| {
            evidence.native_effect == *effect && evidence.reporting_unit == request.reporting_unit
        });
    let evidence =
        candidates
            .next()
            .cloned()
            .ok_or_else(|| EconomicsUnavailable::MissingValuation {
                unit: effect.currency_id(),
            })?;
    if candidates.next().is_some() {
        return Err(EconomicsUnavailable::AmbiguousValuation {
            unit: effect.currency_id(),
        });
    }
    if evidence.valued_at_ns > request.requested_at_ns
        || evidence
            .valid_until_ns
            .is_some_and(|valid_until_ns| valid_until_ns < request.requested_at_ns)
        || evidence.route_id.is_none()
    {
        return Err(EconomicsUnavailable::ValuationEvidenceMismatch);
    }
    Ok(evidence)
}

fn update_valid_until(target: &mut u64, candidate: u64) {
    *target = (*target).min(candidate);
}
