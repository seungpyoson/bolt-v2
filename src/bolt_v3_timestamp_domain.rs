//! Shared timestamp-domain wrappers for bolt-v3 runtime evidence.
//!
//! Each clock domain has its own type, so cross-domain comparisons fail during
//! compilation instead of becoming runtime `None` values.
//!
//! ```compile_fail
//! use bolt_v2::bolt_v3_timestamp_domain::{LocalReceiveMs, VenueEventMs};
//!
//! let _ = VenueEventMs::new(1_000) < LocalReceiveMs::new(1_000);
//! ```

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VenueEventMs(u64);

impl VenueEventMs {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn saturating_duration_since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalReceiveMs(u64);

impl LocalReceiveMs {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn saturating_duration_since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NtStrategyClockMs(u64);

impl NtStrategyClockMs {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn saturating_duration_since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }

    pub const fn saturating_duration_since_venue_event(self, earlier: VenueEventMs) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_domain_timestamps_compare() {
        let earlier = VenueEventMs::new(1_000);
        let later = VenueEventMs::new(1_500);

        assert!(later > earlier);
        assert_eq!(later.saturating_duration_since(earlier), 500);
    }

    #[test]
    fn domain_timestamps_expose_raw_values() {
        let event = VenueEventMs::new(1_000);
        let receive = LocalReceiveMs::new(1_001);
        let strategy = NtStrategyClockMs::new(1_002);

        assert_eq!(event.value(), 1_000);
        assert_eq!(receive.value(), 1_001);
        assert_eq!(strategy.value(), 1_002);
    }

    #[test]
    fn strategy_clock_age_from_venue_event_clamps_venue_leading_skew() {
        let venue_leading_event = VenueEventMs::new(1_005);
        let strategy_clock = NtStrategyClockMs::new(1_000);

        assert_eq!(
            strategy_clock.saturating_duration_since_venue_event(venue_leading_event),
            0
        );
    }
}
