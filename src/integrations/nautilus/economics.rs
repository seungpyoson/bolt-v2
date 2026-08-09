use nautilus_model::{
    enums::{LiquiditySide as NautilusLiquiditySide, OrderSide as NautilusOrderSide},
    identifiers::InstrumentId,
    types::{Currency, Price, Quantity},
};

use crate::economics::{
    CurrencyId, EconomicsError, EconomicsInstrumentId, EconomicsQuoteRequest, LiquidityRole,
    OrderSide,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NautilusEconomicsAdapterError {
    MissingOrderSide,
    MissingLiquiditySide,
    InvalidEconomics(EconomicsError),
}

impl std::fmt::Display for NautilusEconomicsAdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingOrderSide => f.write_str("Nautilus order side is unspecified"),
            Self::MissingLiquiditySide => f.write_str("Nautilus liquidity side is unspecified"),
            Self::InvalidEconomics(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for NautilusEconomicsAdapterError {}

impl From<EconomicsError> for NautilusEconomicsAdapterError {
    fn from(value: EconomicsError) -> Self {
        Self::InvalidEconomics(value)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NautilusEconomicsIntent<'a> {
    pub instrument_id: InstrumentId,
    pub settlement_currency: &'a Currency,
    pub side: NautilusOrderSide,
    pub liquidity_side: NautilusLiquiditySide,
    pub price: Price,
    pub quantity: Quantity,
    pub requested_at_ns: u64,
    pub max_source_age_ns: u64,
}

pub fn economics_request_from_nautilus(
    intent: NautilusEconomicsIntent<'_>,
) -> Result<EconomicsQuoteRequest, NautilusEconomicsAdapterError> {
    let side = match intent.side {
        NautilusOrderSide::Buy => OrderSide::Buy,
        NautilusOrderSide::Sell => OrderSide::Sell,
        NautilusOrderSide::NoOrderSide => {
            return Err(NautilusEconomicsAdapterError::MissingOrderSide);
        }
    };
    let liquidity_role = match intent.liquidity_side {
        NautilusLiquiditySide::Maker => LiquidityRole::Maker,
        NautilusLiquiditySide::Taker => LiquidityRole::Taker,
        NautilusLiquiditySide::NoLiquiditySide => {
            return Err(NautilusEconomicsAdapterError::MissingLiquiditySide);
        }
    };
    let request = EconomicsQuoteRequest {
        instrument_id: EconomicsInstrumentId::try_new(intent.instrument_id.to_string())?,
        settlement_currency: CurrencyId::try_new(intent.settlement_currency.code.to_string())?,
        side,
        liquidity_role,
        price: intent.price.as_decimal(),
        quantity: intent.quantity.as_decimal(),
        requested_at_ns: intent.requested_at_ns,
        max_source_age_ns: intent.max_source_age_ns,
    };
    request.validate()?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use nautilus_model::{
        enums::{CurrencyType, LiquiditySide as NautilusLiquiditySide},
        identifiers::{InstrumentId, Symbol, Venue},
        types::{Currency, Price, Quantity},
    };
    use rust_decimal::Decimal;

    use super::*;

    fn usd() -> Currency {
        Currency::new_checked("USD", 2, 840, "US Dollar", CurrencyType::Fiat)
            .expect("currency fixture should be valid")
    }

    #[test]
    fn nautilus_intent_maps_to_the_neutral_economics_request() {
        let request = economics_request_from_nautilus(NautilusEconomicsIntent {
            instrument_id: InstrumentId::new(Symbol::new("BTC-USD"), Venue::new("SIM")),
            settlement_currency: &usd(),
            side: NautilusOrderSide::Buy,
            liquidity_side: NautilusLiquiditySide::Taker,
            price: Price::new(100.25, 2),
            quantity: Quantity::new(3.0, 1),
            requested_at_ns: 1_000,
            max_source_age_ns: 100,
        })
        .expect("valid Nautilus intent should adapt");

        assert_eq!(request.instrument_id.as_str(), "BTC-USD.SIM");
        assert_eq!(request.settlement_currency.as_str(), "USD");
        assert_eq!(request.side, OrderSide::Buy);
        assert_eq!(request.liquidity_role, LiquidityRole::Taker);
        assert_eq!(request.price, Decimal::new(10_025, 2));
        assert_eq!(request.quantity, Decimal::new(30, 1));
    }

    #[test]
    fn nautilus_intent_rejects_unspecified_execution_facts() {
        let currency = usd();
        let intent = NautilusEconomicsIntent {
            instrument_id: InstrumentId::new(Symbol::new("BTC-USD"), Venue::new("SIM")),
            settlement_currency: &currency,
            side: NautilusOrderSide::NoOrderSide,
            liquidity_side: NautilusLiquiditySide::Taker,
            price: Price::new(100.0, 1),
            quantity: Quantity::new(1.0, 1),
            requested_at_ns: 1_000,
            max_source_age_ns: 100,
        };
        assert_eq!(
            economics_request_from_nautilus(intent),
            Err(NautilusEconomicsAdapterError::MissingOrderSide)
        );

        assert_eq!(
            economics_request_from_nautilus(NautilusEconomicsIntent {
                side: NautilusOrderSide::Buy,
                liquidity_side: NautilusLiquiditySide::NoLiquiditySide,
                ..intent
            }),
            Err(NautilusEconomicsAdapterError::MissingLiquiditySide)
        );
    }
}
