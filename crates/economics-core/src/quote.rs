use rust_decimal::Decimal;

use crate::{
    CurrencyId, DebitRiskBound, EconomicsError, EconomicsInstrumentId, EstimatedEffect,
    LiquidityRole, OrderSide, QuoteHealth, SourceIdentity, ValuationRate,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EconomicsQuoteRequest {
    pub instrument_id: EconomicsInstrumentId,
    pub settlement_currency: CurrencyId,
    pub side: OrderSide,
    pub liquidity_role: LiquidityRole,
    pub price: Decimal,
    pub quantity: Decimal,
    pub requested_at_ns: u64,
    pub max_source_age_ns: u64,
}

impl EconomicsQuoteRequest {
    pub fn validate(&self) -> Result<(), EconomicsError> {
        if self.price <= Decimal::ZERO {
            return Err(EconomicsError::NonPositiveValue { field: "price" });
        }
        if self.quantity <= Decimal::ZERO {
            return Err(EconomicsError::NonPositiveValue { field: "quantity" });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EconomicsQuote {
    pub source: SourceIdentity,
    pub observed_at_ns: u64,
    pub health: QuoteHealth,
    pub effects: Vec<EstimatedEffect>,
    pub debit_risk_bounds: Vec<DebitRiskBound>,
    pub valuations: Vec<ValuationRate>,
}

impl EconomicsQuote {
    pub fn validate_for(&self, request: &EconomicsQuoteRequest) -> Result<(), EconomicsError> {
        request.validate()?;
        if !self.health.is_healthy() {
            return Err(EconomicsError::QuoteUnavailable {
                health: self.health,
            });
        }
        validate_source_time(self.observed_at_ns, request)?;
        for effect in &self.effects {
            effect.validate()?;
            validate_source_time(effect.observed_at_ns, request)?;
        }
        for bound in &self.debit_risk_bounds {
            bound.validate()?;
            validate_source_time(bound.observed_at_ns, request)?;
        }
        for valuation in &self.valuations {
            valuation.validate()?;
            validate_source_time(valuation.observed_at_ns, request)?;
        }
        Ok(())
    }
}

fn validate_source_time(
    observed_at_ns: u64,
    request: &EconomicsQuoteRequest,
) -> Result<(), EconomicsError> {
    let age = request
        .requested_at_ns
        .checked_sub(observed_at_ns)
        .ok_or(EconomicsError::FutureDatedSource)?;
    if age > request.max_source_age_ns {
        return Err(EconomicsError::StaleSource);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectDirection, EffectGuarantee, SourceIdentity};

    fn request() -> EconomicsQuoteRequest {
        EconomicsQuoteRequest {
            instrument_id: EconomicsInstrumentId::try_new("instrument")
                .expect("instrument fixture should be canonical"),
            settlement_currency: CurrencyId::try_new("USD")
                .expect("currency fixture should be canonical"),
            side: OrderSide::Buy,
            liquidity_role: LiquidityRole::Taker,
            price: Decimal::ONE,
            quantity: Decimal::ONE,
            requested_at_ns: 1_000,
            max_source_age_ns: 100,
        }
    }

    fn effect(observed_at_ns: u64) -> EstimatedEffect {
        EstimatedEffect {
            currency: CurrencyId::try_new("USD").expect("currency fixture should be canonical"),
            amount: Decimal::ONE,
            direction: EffectDirection::Debit,
            guarantee: EffectGuarantee::Guaranteed,
            source: SourceIdentity::try_new("schedule")
                .expect("source fixture should be canonical"),
            observed_at_ns,
        }
    }

    #[test]
    fn quote_rejects_unhealthy_future_and_stale_required_economics() {
        let unhealthy = EconomicsQuote {
            source: SourceIdentity::try_new("schedule")
                .expect("source fixture should be canonical"),
            observed_at_ns: 1_000,
            health: QuoteHealth::Unsupported,
            effects: vec![effect(1_000)],
            debit_risk_bounds: Vec::new(),
            valuations: Vec::new(),
        };
        assert!(matches!(
            unhealthy.validate_for(&request()),
            Err(EconomicsError::QuoteUnavailable { .. })
        ));

        for (observed_at_ns, expected) in [
            (1_001, EconomicsError::FutureDatedSource),
            (899, EconomicsError::StaleSource),
        ] {
            let quote = EconomicsQuote {
                source: SourceIdentity::try_new("schedule")
                    .expect("source fixture should be canonical"),
                observed_at_ns,
                health: QuoteHealth::Healthy,
                effects: vec![effect(observed_at_ns)],
                debit_risk_bounds: Vec::new(),
                valuations: Vec::new(),
            };
            assert_eq!(quote.validate_for(&request()), Err(expected));
        }
    }

    #[test]
    fn healthy_fee_free_quote_is_valid_when_its_authority_is_fresh() {
        let quote = EconomicsQuote {
            source: SourceIdentity::try_new("fee-free-schedule")
                .expect("source fixture should be canonical"),
            observed_at_ns: 1_000,
            health: QuoteHealth::Healthy,
            effects: Vec::new(),
            debit_risk_bounds: Vec::new(),
            valuations: Vec::new(),
        };

        assert_eq!(quote.validate_for(&request()), Ok(()));
    }
}
