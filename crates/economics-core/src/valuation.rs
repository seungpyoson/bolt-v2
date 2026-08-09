use rust_decimal::Decimal;

use crate::{CurrencyId, EconomicsError, SourceIdentity};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValuationRate {
    pub from: CurrencyId,
    pub to: CurrencyId,
    pub to_units_per_from_unit: Decimal,
    pub source: SourceIdentity,
    pub observed_at_ns: u64,
}

impl ValuationRate {
    pub fn validate(&self) -> Result<(), EconomicsError> {
        if self.to_units_per_from_unit <= Decimal::ZERO {
            return Err(EconomicsError::NonPositiveValue {
                field: "valuation_rate",
            });
        }
        Ok(())
    }
}
