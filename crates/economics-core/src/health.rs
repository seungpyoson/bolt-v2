use crate::EconomicsError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EconomicsCapabilityHealth {
    required_valid_until_ns: u64,
}

impl EconomicsCapabilityHealth {
    pub const fn quote_only(required_valid_until_ns: u64) -> Self {
        Self {
            required_valid_until_ns,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_required_capability_blocks_admission() {
        let health = EconomicsCapabilityHealth::quote_only(999);
        assert_eq!(
            health.allows_admission(1_000),
            Err(EconomicsError::RequiredCapabilityStale {
                valid_until_ns: 999
            })
        );
    }
}
