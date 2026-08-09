use rust_decimal::Decimal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EconomicsError {
    EmptyIdentifier { field: &'static str },
    NonCanonicalIdentifier { field: &'static str },
    NonPositiveValue { field: &'static str },
    QuoteUnavailable { health: crate::QuoteHealth },
    FutureDatedSource,
    StaleSource,
    MissingValuation { from: CurrencyId, to: CurrencyId },
    ContradictoryValuation { from: CurrencyId, to: CurrencyId },
    ContradictoryEffect,
    ArithmeticOverflow,
}

impl std::fmt::Display for EconomicsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyIdentifier { field } => write!(f, "economics {field} is empty"),
            Self::NonCanonicalIdentifier { field } => {
                write!(f, "economics {field} is not canonical")
            }
            Self::NonPositiveValue { field } => {
                write!(f, "economics {field} must be positive")
            }
            Self::QuoteUnavailable { health } => {
                write!(f, "economics quote is unavailable: {health:?}")
            }
            Self::FutureDatedSource => write!(f, "economics source is future-dated"),
            Self::StaleSource => write!(f, "economics source is stale"),
            Self::MissingValuation { from, to } => {
                write!(f, "economics valuation is missing for {from} -> {to}")
            }
            Self::ContradictoryValuation { from, to } => {
                write!(f, "economics valuation is contradictory for {from} -> {to}")
            }
            Self::ContradictoryEffect => {
                write!(
                    f,
                    "economics effect direction contradicts its guarantee class"
                )
            }
            Self::ArithmeticOverflow => write!(f, "economics arithmetic overflowed"),
        }
    }
}

impl std::error::Error for EconomicsError {}

fn validate_identifier(
    field: &'static str,
    value: impl Into<String>,
) -> Result<String, EconomicsError> {
    let value = value.into();
    if value.is_empty() {
        return Err(EconomicsError::EmptyIdentifier { field });
    }
    if value.trim() != value || value.chars().any(char::is_control) {
        return Err(EconomicsError::NonCanonicalIdentifier { field });
    }
    Ok(value)
}

macro_rules! validated_identifier {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn try_new(value: impl Into<String>) -> Result<Self, EconomicsError> {
                validate_identifier($field, value).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

validated_identifier!(CurrencyId, "currency_id");
validated_identifier!(EconomicsInstrumentId, "instrument_id");
validated_identifier!(SourceIdentity, "source_identity");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiquidityRole {
    Maker,
    Taker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectDirection {
    Debit,
    Credit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectGuarantee {
    Guaranteed,
    ConditionalDebitBound,
    Forecast,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EstimatedEffect {
    pub currency: CurrencyId,
    pub amount: Decimal,
    pub direction: EffectDirection,
    pub guarantee: EffectGuarantee,
    pub source: SourceIdentity,
    pub observed_at_ns: u64,
}

impl EstimatedEffect {
    pub fn validate(&self) -> Result<(), EconomicsError> {
        if self.amount <= Decimal::ZERO {
            return Err(EconomicsError::NonPositiveValue {
                field: "estimated_effect.amount",
            });
        }
        if matches!(
            (self.direction, self.guarantee),
            (
                EffectDirection::Credit,
                EffectGuarantee::ConditionalDebitBound
            ) | (EffectDirection::Debit, EffectGuarantee::Forecast)
        ) {
            return Err(EconomicsError::ContradictoryEffect);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebitRiskBound {
    pub currency: CurrencyId,
    pub amount: Decimal,
    pub source: SourceIdentity,
    pub observed_at_ns: u64,
}

impl DebitRiskBound {
    pub fn validate(&self) -> Result<(), EconomicsError> {
        if self.amount <= Decimal::ZERO {
            return Err(EconomicsError::NonPositiveValue {
                field: "debit_risk_bound.amount",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_reject_empty_padded_and_control_values() {
        assert!(matches!(
            CurrencyId::try_new(""),
            Err(EconomicsError::EmptyIdentifier { .. })
        ));
        assert!(matches!(
            EconomicsInstrumentId::try_new(" instrument "),
            Err(EconomicsError::NonCanonicalIdentifier { .. })
        ));
        assert!(matches!(
            SourceIdentity::try_new("source\n"),
            Err(EconomicsError::NonCanonicalIdentifier { .. })
        ));
        assert_eq!(
            CurrencyId::try_new("USDC")
                .expect("canonical currency should construct")
                .as_str(),
            "USDC"
        );
    }
}
