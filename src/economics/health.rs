use super::EconomicsUnavailable;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EconomicsCapabilityHealth {
    required_valid_until_ns: u64,
    forecast_valid_until_ns: Option<u64>,
}

impl EconomicsCapabilityHealth {
    pub fn quote_only(required_valid_until_ns: u64, forecast_valid_until_ns: Option<u64>) -> Self {
        Self {
            required_valid_until_ns,
            forecast_valid_until_ns,
        }
    }

    pub fn allows_admission(&self, now_ns: u64) -> Result<(), EconomicsUnavailable> {
        if self.required_valid_until_ns < now_ns {
            return Err(EconomicsUnavailable::RequiredCapabilityStale {
                valid_until_ns: self.required_valid_until_ns,
            });
        }
        Ok(())
    }

    pub fn forecast_available(&self, now_ns: u64) -> bool {
        self.forecast_valid_until_ns
            .is_some_and(|valid_until_ns| valid_until_ns >= now_ns)
    }

    pub fn allows_live_execution(&self, now_ns: u64) -> Result<(), EconomicsUnavailable> {
        self.allows_admission(now_ns)?;
        Err(EconomicsUnavailable::ActualAccountingUnavailable)
    }
}
