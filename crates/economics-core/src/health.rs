#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteHealth {
    Healthy,
    MissingRequiredInput,
    Stale,
    Contradictory,
    Unsupported,
    Unvalued,
}

impl QuoteHealth {
    pub const fn is_healthy(self) -> bool {
        matches!(self, Self::Healthy)
    }

    pub const fn combine(self, other: Self) -> Self {
        if self.severity() >= other.severity() {
            self
        } else {
            other
        }
    }

    const fn severity(self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::MissingRequiredInput => 1,
            Self::Stale => 2,
            Self::Unvalued => 3,
            Self::Unsupported => 4,
            Self::Contradictory => 5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_combination_preserves_the_most_conservative_state() {
        assert_eq!(
            QuoteHealth::Healthy.combine(QuoteHealth::Stale),
            QuoteHealth::Stale
        );
        assert_eq!(
            QuoteHealth::Unsupported.combine(QuoteHealth::Contradictory),
            QuoteHealth::Contradictory
        );
    }
}
