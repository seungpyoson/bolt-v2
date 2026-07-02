//! Shared timestamp-domain comparisons for bolt-v3 runtime evidence.

use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampDomain {
    VenueEvent,
    LocalReceive,
    NtStrategyClock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DomainTimestampMs {
    domain: TimestampDomain,
    value: u64,
}

impl DomainTimestampMs {
    pub const fn venue_event(value: u64) -> Self {
        Self {
            domain: TimestampDomain::VenueEvent,
            value,
        }
    }

    pub const fn local_receive(value: u64) -> Self {
        Self {
            domain: TimestampDomain::LocalReceive,
            value,
        }
    }

    pub const fn nt_strategy_clock(value: u64) -> Self {
        Self {
            domain: TimestampDomain::NtStrategyClock,
            value,
        }
    }

    pub const fn domain(self) -> TimestampDomain {
        self.domain
    }

    pub const fn value(self) -> u64 {
        self.value
    }

    pub fn cmp_same_domain(self, other: Self) -> Option<Ordering> {
        (self.domain == other.domain).then(|| self.value.cmp(&other.value))
    }

    pub fn eq_same_domain(self, other: Self) -> Option<bool> {
        self.cmp_same_domain(other)
            .map(|ordering| ordering == Ordering::Equal)
    }

    pub fn lt_same_domain(self, other: Self) -> Option<bool> {
        self.cmp_same_domain(other)
            .map(|ordering| ordering == Ordering::Less)
    }

    pub fn saturating_duration_since_same_domain(self, earlier: Self) -> Option<u64> {
        (self.domain == earlier.domain).then(|| self.value.saturating_sub(earlier.value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_domain_timestamps_compare() {
        let earlier = DomainTimestampMs::venue_event(1_000);
        let later = DomainTimestampMs::venue_event(1_500);

        assert_eq!(later.cmp_same_domain(earlier), Some(Ordering::Greater));
        assert_eq!(
            later.saturating_duration_since_same_domain(earlier),
            Some(500)
        );
    }

    #[test]
    fn cross_domain_timestamps_do_not_compare() {
        let event = DomainTimestampMs::venue_event(1_000);
        let receive = DomainTimestampMs::local_receive(1_001);
        let strategy = DomainTimestampMs::nt_strategy_clock(1_002);

        assert_eq!(event.cmp_same_domain(receive), None);
        assert_eq!(receive.lt_same_domain(strategy), None);
        assert_eq!(strategy.saturating_duration_since_same_domain(event), None);
    }
}
