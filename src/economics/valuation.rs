use std::collections::HashSet;

use rust_decimal::Decimal;

use super::{
    Currency, EconomicsUnavailable, SignedNativeEffect, ValuationEvidence, ValuationRoute,
};

pub fn value_with_route(
    effect: &SignedNativeEffect,
    reporting_unit: &Currency,
    route: Option<&ValuationRoute>,
    valued_at_ns: u64,
) -> Result<ValuationEvidence, EconomicsUnavailable> {
    if effect.currency_id() == *reporting_unit {
        return Ok(ValuationEvidence {
            native_effect: effect.clone(),
            normalized_amount: effect.amount(),
            reporting_unit: reporting_unit.clone(),
            route_id: None,
            legs: Vec::new(),
            source_snapshot_ids: Vec::new(),
            valued_at_ns,
            valid_until_ns: None,
        });
    }

    let route = route.ok_or_else(|| EconomicsUnavailable::MissingValuationRoute {
        from: effect.currency_id(),
        to: reporting_unit.clone(),
    })?;
    if route.from_unit != effect.currency_id() || route.to_currency != *reporting_unit {
        return Err(EconomicsUnavailable::DisconnectedValuationRoute {
            route_id: route.route_id.clone(),
        });
    }
    if route.valid_until_ns < valued_at_ns {
        return Err(EconomicsUnavailable::StaleValuation {
            route_id: route.route_id.clone(),
        });
    }

    let mut current = route.from_unit.clone();
    let mut visited = HashSet::from([current.clone()]);
    let mut rate = Decimal::ONE;
    let mut valid_until_ns = route.valid_until_ns;
    let mut source_snapshot_ids = Vec::with_capacity(route.legs.len());
    for leg in &route.legs {
        if leg.from_unit != current {
            return Err(EconomicsUnavailable::DisconnectedValuationRoute {
                route_id: route.route_id.clone(),
            });
        }
        if !visited.insert(leg.to_unit.clone()) {
            return Err(EconomicsUnavailable::CyclicValuationRoute {
                route_id: route.route_id.clone(),
            });
        }
        if leg.rate <= Decimal::ZERO {
            return Err(EconomicsUnavailable::InvalidValuationRate {
                route_id: route.route_id.clone(),
            });
        }
        if leg.observed_at_ns > leg.fetched_at_ns
            || leg.fetched_at_ns > leg.valid_until_ns
            || leg.fetched_at_ns > valued_at_ns
            || leg.valid_until_ns < valued_at_ns
        {
            return Err(EconomicsUnavailable::StaleValuation {
                route_id: route.route_id.clone(),
            });
        }
        rate = rate
            .checked_mul(leg.rate)
            .ok_or(EconomicsUnavailable::InvalidDecimal)?;
        valid_until_ns = valid_until_ns.min(leg.valid_until_ns);
        source_snapshot_ids.push(leg.source_snapshot_id.clone());
        current = leg.to_unit.clone();
    }
    if route.legs.is_empty() || current != route.to_currency {
        return Err(EconomicsUnavailable::DisconnectedValuationRoute {
            route_id: route.route_id.clone(),
        });
    }

    Ok(ValuationEvidence {
        native_effect: effect.clone(),
        normalized_amount: effect
            .amount()
            .checked_mul(rate)
            .ok_or(EconomicsUnavailable::InvalidDecimal)?,
        reporting_unit: reporting_unit.clone(),
        route_id: Some(route.route_id.clone()),
        legs: route.legs.clone(),
        source_snapshot_ids,
        valued_at_ns,
        valid_until_ns: Some(valid_until_ns),
    })
}
