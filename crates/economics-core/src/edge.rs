use rust_decimal::Decimal;

use crate::{
    CurrencyId, EconomicsError, EconomicsQuote, EconomicsQuoteRequest, EffectDirection,
    EffectGuarantee, ValuationRate,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldedEdge {
    pub gross_edge: Decimal,
    pub authorization_effect: Decimal,
    pub net_authorization_edge: Decimal,
}

pub fn fold_core_edge(
    gross_edge: Decimal,
    request: &EconomicsQuoteRequest,
    quote: &EconomicsQuote,
) -> Result<FoldedEdge, EconomicsError> {
    quote.validate_for(request)?;
    let mut authorization_effect = Decimal::ZERO;
    for effect in &quote.effects {
        if effect.guarantee == EffectGuarantee::Forecast {
            continue;
        }
        let amount = value_in(
            effect.amount,
            &effect.currency,
            &request.settlement_currency,
            &quote.valuations,
        )?;
        authorization_effect = match effect.direction {
            EffectDirection::Debit => authorization_effect
                .checked_sub(amount)
                .ok_or(EconomicsError::ArithmeticOverflow)?,
            EffectDirection::Credit => authorization_effect
                .checked_add(amount)
                .ok_or(EconomicsError::ArithmeticOverflow)?,
        };
    }
    for bound in &quote.debit_risk_bounds {
        let amount = value_in(
            bound.amount,
            &bound.currency,
            &request.settlement_currency,
            &quote.valuations,
        )?;
        authorization_effect = authorization_effect
            .checked_sub(amount)
            .ok_or(EconomicsError::ArithmeticOverflow)?;
    }
    let net_authorization_edge = gross_edge
        .checked_add(authorization_effect)
        .ok_or(EconomicsError::ArithmeticOverflow)?;
    Ok(FoldedEdge {
        gross_edge,
        authorization_effect,
        net_authorization_edge,
    })
}

fn value_in(
    amount: Decimal,
    from: &CurrencyId,
    to: &CurrencyId,
    valuations: &[ValuationRate],
) -> Result<Decimal, EconomicsError> {
    if from == to {
        return Ok(amount);
    }
    let mut matching = valuations
        .iter()
        .filter(|valuation| &valuation.from == from && &valuation.to == to);
    let rate = matching
        .next()
        .ok_or_else(|| EconomicsError::MissingValuation {
            from: from.clone(),
            to: to.clone(),
        })?;
    if matching.next().is_some() {
        return Err(EconomicsError::ContradictoryValuation {
            from: from.clone(),
            to: to.clone(),
        });
    }
    amount
        .checked_mul(rate.to_units_per_from_unit)
        .ok_or(EconomicsError::ArithmeticOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DebitRiskBound, EconomicsInstrumentId, EstimatedEffect, LiquidityRole, OrderSide,
        QuoteHealth, SourceIdentity,
    };

    fn currency(value: &str) -> CurrencyId {
        CurrencyId::try_new(value).expect("currency fixture should be canonical")
    }

    fn source() -> SourceIdentity {
        SourceIdentity::try_new("schedule").expect("source fixture should be canonical")
    }

    fn request() -> EconomicsQuoteRequest {
        EconomicsQuoteRequest {
            instrument_id: EconomicsInstrumentId::try_new("instrument")
                .expect("instrument fixture should be canonical"),
            settlement_currency: currency("USD"),
            side: OrderSide::Buy,
            liquidity_role: LiquidityRole::Taker,
            price: Decimal::ONE,
            quantity: Decimal::ONE,
            requested_at_ns: 1_000,
            max_source_age_ns: 100,
        }
    }

    #[test]
    fn fold_subtracts_debits_and_bounds_but_ignores_forecast_credits() {
        let quote = EconomicsQuote {
            source: source(),
            observed_at_ns: 1_000,
            health: QuoteHealth::Healthy,
            effects: vec![
                EstimatedEffect {
                    currency: currency("USD"),
                    amount: Decimal::new(2, 0),
                    direction: EffectDirection::Debit,
                    guarantee: EffectGuarantee::Guaranteed,
                    source: source(),
                    observed_at_ns: 1_000,
                },
                EstimatedEffect {
                    currency: currency("USD"),
                    amount: Decimal::new(50, 0),
                    direction: EffectDirection::Credit,
                    guarantee: EffectGuarantee::Forecast,
                    source: source(),
                    observed_at_ns: 1_000,
                },
            ],
            debit_risk_bounds: vec![DebitRiskBound {
                currency: currency("USD"),
                amount: Decimal::ONE,
                source: source(),
                observed_at_ns: 1_000,
            }],
            valuations: Vec::new(),
        };

        assert_eq!(
            fold_core_edge(Decimal::new(10, 0), &request(), &quote),
            Ok(FoldedEdge {
                gross_edge: Decimal::new(10, 0),
                authorization_effect: Decimal::new(-3, 0),
                net_authorization_edge: Decimal::new(7, 0),
            })
        );
    }

    #[test]
    fn fold_requires_exactly_one_valuation_for_foreign_native_units() {
        let effect = EstimatedEffect {
            currency: currency("TOKEN"),
            amount: Decimal::new(2, 0),
            direction: EffectDirection::Debit,
            guarantee: EffectGuarantee::Guaranteed,
            source: source(),
            observed_at_ns: 1_000,
        };
        let quote = EconomicsQuote {
            source: source(),
            observed_at_ns: 1_000,
            health: QuoteHealth::Healthy,
            effects: vec![effect],
            debit_risk_bounds: Vec::new(),
            valuations: Vec::new(),
        };
        assert!(matches!(
            fold_core_edge(Decimal::new(10, 0), &request(), &quote),
            Err(EconomicsError::MissingValuation { .. })
        ));

        let rate = ValuationRate {
            from: currency("TOKEN"),
            to: currency("USD"),
            to_units_per_from_unit: Decimal::new(2, 0),
            source: source(),
            observed_at_ns: 1_000,
        };
        let valued = EconomicsQuote {
            valuations: vec![rate.clone()],
            ..quote.clone()
        };
        assert_eq!(
            fold_core_edge(Decimal::new(10, 0), &request(), &valued),
            Ok(FoldedEdge {
                gross_edge: Decimal::new(10, 0),
                authorization_effect: Decimal::new(-4, 0),
                net_authorization_edge: Decimal::new(6, 0),
            })
        );

        let contradictory = EconomicsQuote {
            valuations: vec![rate.clone(), rate],
            ..quote
        };
        assert!(matches!(
            fold_core_edge(Decimal::new(10, 0), &request(), &contradictory),
            Err(EconomicsError::ContradictoryValuation { .. })
        ));
    }
}
