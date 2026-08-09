use bolt_v2::economics::{
    CurrencyId, EconomicsError, EconomicsInstrumentId, EconomicsQuoteRequest, LiquidityRole,
    OrderSide,
};
use rust_decimal::Decimal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayOrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayLiquidityRole {
    Maker,
    Taker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayEconomicsIntent {
    pub instrument_id: String,
    pub settlement_currency: String,
    pub side: ReplayOrderSide,
    pub liquidity_role: ReplayLiquidityRole,
    pub price: Decimal,
    pub quantity: Decimal,
    pub requested_at_ns: u64,
    pub max_source_age_ns: u64,
}

pub fn economics_request_from_replay(
    intent: ReplayEconomicsIntent,
) -> Result<EconomicsQuoteRequest, EconomicsError> {
    let request = EconomicsQuoteRequest {
        instrument_id: EconomicsInstrumentId::try_new(intent.instrument_id)?,
        settlement_currency: CurrencyId::try_new(intent.settlement_currency)?,
        side: match intent.side {
            ReplayOrderSide::Buy => OrderSide::Buy,
            ReplayOrderSide::Sell => OrderSide::Sell,
        },
        liquidity_role: match intent.liquidity_role {
            ReplayLiquidityRole::Maker => LiquidityRole::Maker,
            ReplayLiquidityRole::Taker => LiquidityRole::Taker,
        },
        price: intent.price,
        quantity: intent.quantity,
        requested_at_ns: intent.requested_at_ns,
        max_source_age_ns: intent.max_source_age_ns,
    };
    request.validate()?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_intent_maps_to_the_same_neutral_request_shape() {
        let request = economics_request_from_replay(ReplayEconomicsIntent {
            instrument_id: "BTC-USD.SIM".to_owned(),
            settlement_currency: "USD".to_owned(),
            side: ReplayOrderSide::Sell,
            liquidity_role: ReplayLiquidityRole::Maker,
            price: Decimal::new(10_025, 2),
            quantity: Decimal::new(30, 1),
            requested_at_ns: 1_000,
            max_source_age_ns: 100,
        })
        .expect("valid replay intent should adapt");

        assert_eq!(request.instrument_id.as_str(), "BTC-USD.SIM");
        assert_eq!(request.settlement_currency.as_str(), "USD");
        assert_eq!(request.side, OrderSide::Sell);
        assert_eq!(request.liquidity_role, LiquidityRole::Maker);
        assert_eq!(request.price, Decimal::new(10_025, 2));
        assert_eq!(request.quantity, Decimal::new(30, 1));
    }

    #[test]
    fn replay_intent_rejects_non_positive_execution_values() {
        let error = economics_request_from_replay(ReplayEconomicsIntent {
            instrument_id: "BTC-USD.SIM".to_owned(),
            settlement_currency: "USD".to_owned(),
            side: ReplayOrderSide::Buy,
            liquidity_role: ReplayLiquidityRole::Taker,
            price: Decimal::ZERO,
            quantity: Decimal::ONE,
            requested_at_ns: 1_000,
            max_source_age_ns: 100,
        })
        .expect_err("zero replay price must fail closed");

        assert_eq!(
            error,
            EconomicsError::NonPositiveValue { field: "price" }
        );
    }
}
