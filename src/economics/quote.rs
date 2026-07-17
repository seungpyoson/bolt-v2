use std::collections::HashSet;

use rust_decimal::Decimal;

use super::{
    AdmissionTreatment, EconomicQuote, EconomicQuoteRequest, EconomicsUnavailable,
    EstimatedEconomicComponent, SignedNativeEffect, ValuationEvidence, value_with_route,
};

pub fn validate_and_aggregate_quote(
    request: &EconomicQuoteRequest,
    components: Vec<EstimatedEconomicComponent>,
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
    if components.is_empty() {
        return Err(EconomicsUnavailable::EmptyQuote);
    }

    let mut component_ids = HashSet::new();
    let mut accepted = Vec::with_capacity(components.len());
    let mut normalizations = Vec::new();
    let mut core_total = Decimal::ZERO;
    let mut forecast_total = Decimal::ZERO;
    let mut forecast_complete = true;
    let mut required_valid_until_ns: Option<u64> = None;

    for mut component in components {
        if !component_ids.insert(component.component_id.clone()) {
            return Err(EconomicsUnavailable::DuplicateComponent {
                component_id: component.component_id,
            });
        }
        validate_source_timeline(request, &component)?;
        if component.source.valid_until_ns < request.requested_at_ns {
            if component.admission_treatment == AdmissionTreatment::ForecastOnly {
                forecast_complete = false;
                continue;
            }
            return Err(EconomicsUnavailable::StaleSource {
                source_id: component.source.source_id,
            });
        }

        let point_valuation = resolve_valuation(
            &component.point_effect,
            component.normalized.as_ref(),
            valuations,
            request,
        )?;
        forecast_total += point_valuation.normalized_amount;
        component.normalized = Some(point_valuation.clone());
        normalizations.push(point_valuation);

        match component.admission_treatment {
            AdmissionTreatment::GuaranteedConditionalOnAction => {
                core_total += component
                    .normalized
                    .as_ref()
                    .expect("point normalization was assigned")
                    .normalized_amount;
                update_valid_until(
                    &mut required_valid_until_ns,
                    component.source.valid_until_ns,
                );
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
                core_total += bound_valuation.normalized_amount;
                if let Some(valid_until_ns) = bound_valuation.valid_until_ns {
                    update_valid_until(&mut required_valid_until_ns, valid_until_ns);
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

    if accepted.is_empty() {
        return Err(EconomicsUnavailable::EmptyQuote);
    }
    let valid_until_ns = required_valid_until_ns.unwrap_or(request.requested_at_ns);
    Ok(EconomicQuote {
        decision_correlation_id: request.decision_correlation_id.clone(),
        requested_at_ns: request.requested_at_ns,
        edge_basis_policy_id: request.edge_basis_policy_id.clone(),
        components: accepted,
        normalizations,
        core_total,
        forecast_total,
        forecast_complete,
        reporting_unit: request.reporting_unit.clone(),
        valid_until_ns,
    })
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
    if effect.unit() == &request.reporting_unit {
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

    let evidence = embedded
        .into_iter()
        .chain(valuations.iter())
        .find(|evidence| {
            evidence.native_effect == *effect && evidence.reporting_unit == request.reporting_unit
        })
        .cloned()
        .ok_or_else(|| EconomicsUnavailable::MissingValuation {
            unit: effect.unit().clone(),
        })?;
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

fn update_valid_until(target: &mut Option<u64>, candidate: u64) {
    *target = Some(target.map_or(candidate, |current| current.min(candidate)));
}
