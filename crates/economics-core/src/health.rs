use crate::EconomicsError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EconomicsCapabilityHealth {
    required_valid_until_ns: u64,
    forecast_valid_until_ns: Option<u64>,
}

impl EconomicsCapabilityHealth {
    pub const fn quote_only(
        required_valid_until_ns: u64,
        forecast_valid_until_ns: Option<u64>,
    ) -> Self {
        Self {
            required_valid_until_ns,
            forecast_valid_until_ns,
        }
    }

    pub fn allows_admission(&self, now_ns: u64) -> Result<(), EconomicsError> {
        if self.required_valid_until_ns < now_ns {
            return Err(EconomicsError::RequiredCapabilityStale {
                valid_until_ns: self.required_valid_until_ns,
            });
        }
        Ok(())
    }

    pub fn forecast_available(&self, now_ns: u64) -> bool {
        self.forecast_valid_until_ns
            .is_some_and(|valid_until_ns| valid_until_ns >= now_ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_required_capability_blocks_while_forecast_health_is_supplemental() {
        let health = EconomicsCapabilityHealth::quote_only(999, None);
        assert_eq!(
            health.allows_admission(1_000),
            Err(EconomicsError::RequiredCapabilityStale {
                valid_until_ns: 999
            })
        );
        assert!(!health.forecast_available(1_000));
    }
}
