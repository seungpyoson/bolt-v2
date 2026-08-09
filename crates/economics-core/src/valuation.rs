use std::collections::HashSet;

use rust_decimal::Decimal;

use crate::{
    CurrencyId, EconomicsError, NativeUnitId, SignedNativeEffect, SnapshotId, SourceIdentity,
    ValuationRouteId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValuationLeg {
    pub from: NativeUnitId,
    pub to: NativeUnitId,
    pub to_units_per_from_unit: Decimal,
    pub source: SourceIdentity,
    pub snapshot_id: SnapshotId,
    pub observed_at_ns: u64,
    pub fetched_at_ns: u64,
    pub valid_until_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValuationRoute {
    pub route_id: ValuationRouteId,
    pub from: NativeUnitId,
    pub to: CurrencyId,
    pub legs: Vec<ValuationLeg>,
    pub valid_until_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValuationEvidence {
    pub native_effect: SignedNativeEffect,
    pub normalized_amount: Decimal,
    pub reporting_currency: CurrencyId,
    pub route_id: Option<ValuationRouteId>,
    pub source_snapshot_ids: Vec<SnapshotId>,
    pub valued_at_ns: u64,
    pub valid_until_ns: Option<u64>,
}

pub fn value_with_routes(
    effect: &SignedNativeEffect,
    reporting_currency: &CurrencyId,
    routes: &[ValuationRoute],
    valued_at_ns: u64,
) -> Result<ValuationEvidence, EconomicsError> {
    let from = effect.unit();
    if from == NativeUnitId::Currency(reporting_currency.clone()) {
        return Ok(ValuationEvidence {
            native_effect: effect.clone(),
            normalized_amount: effect.amount(),
            reporting_currency: reporting_currency.clone(),
            route_id: None,
            source_snapshot_ids: Vec::new(),
            valued_at_ns,
            valid_until_ns: None,
        });
    }

    let mut matching = routes
        .iter()
        .filter(|route| route.from == from && route.to == *reporting_currency);
    let route = matching
        .next()
        .ok_or_else(|| EconomicsError::MissingValuation {
            from: from.clone(),
            to: reporting_currency.clone(),
        })?;
    if matching.next().is_some() {
        return Err(EconomicsError::ContradictoryValuation {
            from,
            to: reporting_currency.clone(),
        });
    }
    validate_route(route, valued_at_ns)?;

    let mut rate = Decimal::ONE;
    let mut valid_until_ns = route.valid_until_ns;
    let mut source_snapshot_ids = Vec::with_capacity(route.legs.len());
    for leg in &route.legs {
        rate = rate
            .checked_mul(leg.to_units_per_from_unit)
            .ok_or(EconomicsError::ArithmeticOverflow)?;
        valid_until_ns = valid_until_ns.min(leg.valid_until_ns);
        source_snapshot_ids.push(leg.snapshot_id.clone());
    }

    Ok(ValuationEvidence {
        native_effect: effect.clone(),
        normalized_amount: effect
            .amount()
            .checked_mul(rate)
            .ok_or(EconomicsError::ArithmeticOverflow)?,
        reporting_currency: reporting_currency.clone(),
        route_id: Some(route.route_id.clone()),
        source_snapshot_ids,
        valued_at_ns,
        valid_until_ns: Some(valid_until_ns),
    })
}

fn validate_route(route: &ValuationRoute, valued_at_ns: u64) -> Result<(), EconomicsError> {
    if route.valid_until_ns < valued_at_ns {
        return Err(EconomicsError::StaleValuation {
            route_id: route.route_id.clone(),
        });
    }
    if route.legs.is_empty() {
        return Err(EconomicsError::InvalidValuationRoute {
            route_id: route.route_id.clone(),
        });
    }
    let mut current = route.from.clone();
    let mut visited = HashSet::from([current.clone()]);
    for leg in &route.legs {
        if leg.from != current
            || leg.to_units_per_from_unit <= Decimal::ZERO
            || leg.observed_at_ns > leg.fetched_at_ns
            || leg.fetched_at_ns > valued_at_ns
            || leg.fetched_at_ns > leg.valid_until_ns
            || leg.valid_until_ns < valued_at_ns
            || !visited.insert(leg.to.clone())
        {
            return Err(EconomicsError::InvalidValuationRoute {
                route_id: route.route_id.clone(),
            });
        }
        current = leg.to.clone();
    }
    if current != NativeUnitId::Currency(route.to.clone()) {
        return Err(EconomicsError::InvalidValuationRoute {
            route_id: route.route_id.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AssetId, InventoryApplication};

    fn currency(value: &str) -> CurrencyId {
        CurrencyId::try_new(value).expect("currency fixture should be canonical")
    }

    fn source(value: &str) -> SourceIdentity {
        SourceIdentity::try_new(value).expect("source fixture should be canonical")
    }

    #[test]
    fn route_values_asset_quantity_without_erasing_native_effect() {
        let asset = AssetId::try_new("BASE").expect("asset fixture should be canonical");
        let effect = SignedNativeEffect::asset(
            Decimal::new(-2, 0),
            asset.clone(),
            InventoryApplication::ApplyOnceToNetPortfolio,
        )
        .expect("non-zero effect should construct");
        let route = ValuationRoute {
            route_id: ValuationRouteId::try_new("base-usdc")
                .expect("route fixture should be canonical"),
            from: NativeUnitId::Asset(asset.clone()),
            to: currency("USDC"),
            legs: vec![ValuationLeg {
                from: NativeUnitId::Asset(asset),
                to: NativeUnitId::Currency(currency("USDC")),
                to_units_per_from_unit: Decimal::new(10, 0),
                source: source("book"),
                snapshot_id: SnapshotId::try_new("book-1")
                    .expect("snapshot fixture should be canonical"),
                observed_at_ns: 900,
                fetched_at_ns: 950,
                valid_until_ns: 1_100,
            }],
            valid_until_ns: 1_100,
        };

        let evidence = value_with_routes(&effect, &currency("USDC"), &[route], 1_000)
            .expect("fresh route should value the effect");
        assert_eq!(evidence.native_effect, effect);
        assert_eq!(evidence.normalized_amount, Decimal::new(-20, 0));
    }

    #[test]
    fn duplicate_and_stale_routes_fail_closed() {
        let effect = SignedNativeEffect::currency(Decimal::ONE, currency("TOKEN"))
            .expect("non-zero effect should construct");
        let route = ValuationRoute {
            route_id: ValuationRouteId::try_new("token-usd")
                .expect("route fixture should be canonical"),
            from: NativeUnitId::Currency(currency("TOKEN")),
            to: currency("USD"),
            legs: vec![ValuationLeg {
                from: NativeUnitId::Currency(currency("TOKEN")),
                to: NativeUnitId::Currency(currency("USD")),
                to_units_per_from_unit: Decimal::ONE,
                source: source("fx"),
                snapshot_id: SnapshotId::try_new("fx-1")
                    .expect("snapshot fixture should be canonical"),
                observed_at_ns: 800,
                fetched_at_ns: 900,
                valid_until_ns: 999,
            }],
            valid_until_ns: 999,
        };
        assert!(matches!(
            value_with_routes(&effect, &currency("USD"), &[route.clone()], 1_000),
            Err(EconomicsError::StaleValuation { .. })
        ));
        assert!(matches!(
            value_with_routes(&effect, &currency("USD"), &[route.clone(), route], 900),
            Err(EconomicsError::ContradictoryValuation { .. })
        ));
    }
}
