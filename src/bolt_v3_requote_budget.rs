//! Shared maker requote-rate throttle.

use std::collections::VecDeque;

const INITIAL_WINDOW_COST: u64 = u64::MIN;

/// Sliding-window maker requote throttle denominated in REST-call cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequoteBudget {
    window_ms: u64,
    max_cost_per_window: u64,
    min_interval_ms: u64,
    emits: VecDeque<(u64, u64)>,
    window_cost: u64,
    last_emit_ms: Option<u64>,
}

impl RequoteBudget {
    /// Construct a throttle with an explicit window cap, window length, and
    /// minimum interval. Callers derive these values from TOML and venue
    /// capability facts; the helper only accounts for already-resolved values.
    pub fn new(max_cost_per_window: u64, window_ms: u64, min_interval_ms: u64) -> Self {
        Self {
            window_ms,
            max_cost_per_window,
            min_interval_ms,
            emits: VecDeque::new(),
            window_cost: INITIAL_WINDOW_COST,
            last_emit_ms: None,
        }
    }

    /// Try to reserve budget for a requote with explicit REST-call `cost` at
    /// `now_ms`. Returns `true` and records the charge when both the interval
    /// and sliding-window budget allow it.
    pub fn try_acquire(&mut self, now_ms: u64, cost: u64) -> bool {
        if cost == 0 || self.window_ms == 0 {
            return false;
        }
        if let Some(last_ms) = self.last_emit_ms
            && now_ms < last_ms
        {
            return false;
        }
        self.evict(now_ms);

        if let Some(last_ms) = self.last_emit_ms
            && now_ms.saturating_sub(last_ms) < self.min_interval_ms
        {
            return false;
        }

        let Some(next_cost) = self.window_cost.checked_add(cost) else {
            return false;
        };
        if next_cost > self.max_cost_per_window {
            return false;
        }

        self.emits.push_back((now_ms, cost));
        self.window_cost = next_cost;
        self.last_emit_ms = Some(now_ms);
        true
    }

    /// Number of granted commands currently counted inside the window.
    pub fn in_window(&self) -> usize {
        self.emits.len()
    }

    /// Total REST-call cost currently counted inside the window.
    pub fn cost_in_window(&self) -> u64 {
        self.window_cost
    }

    fn evict(&mut self, now_ms: u64) {
        if now_ms <= self.window_ms {
            return;
        }
        let cutoff_ms = now_ms - self.window_ms;
        while let Some(&(timestamp_ms, _)) = self.emits.front() {
            if timestamp_ms >= cutoff_ms {
                break;
            }
            if let Some((_, cost)) = self.emits.pop_front() {
                self.window_cost -= cost;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_MINUTE_MS: u64 = 60_000;

    #[test]
    fn burst_beyond_window_budget_is_throttled() {
        let mut budget = RequoteBudget::new(3, ONE_MINUTE_MS, 0);

        let granted = (0..5)
            .filter(|&i| budget.try_acquire(1_000 + i * 100, 1))
            .count();

        assert_eq!(granted, 3);
        assert_eq!(budget.in_window(), 3);
        assert_eq!(budget.cost_in_window(), 3);
    }

    #[test]
    fn min_interval_throttles_back_to_back_requotes() {
        let mut budget = RequoteBudget::new(100, ONE_MINUTE_MS, 500);

        assert!(budget.try_acquire(1_000, 1));
        assert!(!budget.try_acquire(1_400, 1));
        assert!(budget.try_acquire(1_500, 1));
    }

    #[test]
    fn tokens_replenish_as_the_window_slides() {
        let mut budget = RequoteBudget::new(2, ONE_MINUTE_MS, 0);

        assert!(budget.try_acquire(1_000, 1));
        assert!(budget.try_acquire(2_000, 1));
        assert!(!budget.try_acquire(3_000, 1));
        assert!(budget.try_acquire(1_000 + ONE_MINUTE_MS + 1, 1));
    }

    #[test]
    fn timestamp_at_exact_window_edge_remains_counted() {
        let mut budget = RequoteBudget::new(1, ONE_MINUTE_MS, 0);

        assert!(budget.try_acquire(0, 1));
        assert!(!budget.try_acquire(ONE_MINUTE_MS, 1));
        assert_eq!(budget.in_window(), 1);
        assert_eq!(budget.cost_in_window(), 1);

        assert!(budget.try_acquire(ONE_MINUTE_MS + 1, 1));
        assert_eq!(budget.in_window(), 1);
        assert_eq!(budget.cost_in_window(), 1);
    }

    #[test]
    fn disabled_or_zero_cost_inputs_fail_closed() {
        let mut disabled = RequoteBudget::new(0, ONE_MINUTE_MS, 0);
        assert!(!disabled.try_acquire(1_000, 1));
        assert_eq!(disabled.in_window(), 0);
        assert_eq!(disabled.cost_in_window(), 0);

        let mut zero_window = RequoteBudget::new(10, 0, 0);
        assert!(!zero_window.try_acquire(1_000, 1));
        assert_eq!(zero_window.in_window(), 0);
        assert_eq!(zero_window.cost_in_window(), 0);

        let mut budget = RequoteBudget::new(10, ONE_MINUTE_MS, 0);
        assert!(!budget.try_acquire(1_000, 0));
        assert_eq!(budget.in_window(), 0);
        assert_eq!(budget.cost_in_window(), 0);
    }

    #[test]
    fn weighted_cost_counts_rest_calls_not_requotes() {
        let mut budget = RequoteBudget::new(4, ONE_MINUTE_MS, 0);

        assert!(budget.try_acquire(1_000, 2));
        assert!(budget.try_acquire(1_100, 2));
        assert!(!budget.try_acquire(1_200, 2));

        assert_eq!(budget.in_window(), 2);
        assert_eq!(budget.cost_in_window(), 4);
    }

    #[test]
    fn a_single_requote_costlier_than_the_budget_never_grants() {
        let mut budget = RequoteBudget::new(1, ONE_MINUTE_MS, 0);

        assert!(!budget.try_acquire(1_000, 2));
        assert_eq!(budget.in_window(), 0);
        assert_eq!(budget.cost_in_window(), 0);
    }

    #[test]
    fn checked_add_overflow_fails_closed_without_mutating_window() {
        let mut budget = RequoteBudget::new(u64::MAX, ONE_MINUTE_MS, 0);
        let initial_cost = u64::MAX - 1;

        assert!(budget.try_acquire(1_000, initial_cost));
        assert!(!budget.try_acquire(1_001, 2));
        assert_eq!(budget.in_window(), 1);
        assert_eq!(budget.cost_in_window(), initial_cost);
    }

    #[test]
    fn out_of_order_timestamps_are_rejected_without_poisoning_the_window() {
        let mut budget = RequoteBudget::new(2, ONE_MINUTE_MS, 0);

        assert!(budget.try_acquire(10_000, 1));
        assert!(!budget.try_acquire(9_000, 1));
        assert_eq!(budget.in_window(), 1);
        assert_eq!(budget.cost_in_window(), 1);

        assert!(budget.try_acquire(10_001 + ONE_MINUTE_MS, 1));
        assert_eq!(budget.in_window(), 1);
        assert_eq!(budget.cost_in_window(), 1);
    }
}
