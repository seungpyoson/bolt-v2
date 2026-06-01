//! Pure, NautilusTrader-free requote-rate throttle for the binary-oracle maker
//! (W2 slice 4 — requote throttle reconciled to the venue budget).
//!
//! It keeps the maker's requote rate under the venue's published REST budget so
//! the strategy never out-runs the adapter's own rate limiter. The throttle is a
//! sliding-window token check plus a minimum inter-requote interval — pure
//! integer logic over a caller-supplied millisecond clock, with no NT type and no
//! wall-clock read, so it is exhaustively unit-testable.
//!
//! The window cap is resolved by the strategy layer from the venue capability
//! contract (`rate_budget.clob_per_minute`) times a configured
//! `requote_budget_fraction` (reserving headroom for non-requote REST calls); the
//! window length likewise comes from the minute definition in
//! [`crate::bolt_v3_numeric`]. This module receives the already-resolved integer
//! cap, interval, and window so it stays free of floats and hardcoded facts.
//! (The per-batch submit cap from `rate_budget.batch_submit_limit` is applied at
//! the order-submission site in the NT strategy shell, a later slice.)

use std::collections::VecDeque;

/// A sliding-window requote-rate throttle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequoteBudget {
    /// Length of the rate window, in milliseconds (e.g. one minute).
    window_ms: u64,
    /// Maximum requote commands permitted within any `window_ms` window.
    max_per_window: u64,
    /// Minimum gap between two requote commands, in milliseconds.
    min_interval_ms: u64,
    /// Timestamps (ms) of commands still inside the current window, oldest first.
    emits: VecDeque<u64>,
    /// Timestamp (ms) of the most recent granted command, for the min-interval.
    last_emit_ms: Option<u64>,
}

impl RequoteBudget {
    /// A throttle permitting at most `max_per_window` commands per `window_ms`,
    /// no closer together than `min_interval_ms`. Constructed explicitly — the
    /// bolt-v3 surface forbids `Default` — so the caller names the resolved
    /// budget it derived from the venue contract.
    pub fn new(max_per_window: u64, window_ms: u64, min_interval_ms: u64) -> Self {
        Self {
            window_ms,
            max_per_window,
            min_interval_ms,
            emits: VecDeque::new(),
            last_emit_ms: None,
        }
    }

    /// Try to acquire a requote slot as of `now_ms`. Returns `true` — recording
    /// the command — when both the minimum interval since the last command and
    /// the sliding-window budget allow it; `false` (throttled, so the caller
    /// coalesces or drops the requote) otherwise.
    ///
    /// Fail-closed: a `max_per_window` of zero permits nothing.
    pub fn try_acquire(&mut self, now_ms: u64) -> bool {
        self.evict(now_ms);
        if let Some(last_ms) = self.last_emit_ms
            && now_ms.saturating_sub(last_ms) < self.min_interval_ms
        {
            return false;
        }
        if self.emits.len() as u64 >= self.max_per_window {
            return false;
        }
        self.emits.push_back(now_ms);
        self.last_emit_ms = Some(now_ms);
        true
    }

    /// Commands currently counted within the window as of the last
    /// acquire/evict. Read seam for observability.
    pub fn in_window(&self) -> usize {
        self.emits.len()
    }

    fn evict(&mut self, now_ms: u64) {
        let cutoff_ms = now_ms.saturating_sub(self.window_ms);
        while self.emits.front().is_some_and(|&ts_ms| ts_ms < cutoff_ms) {
            let _ = self.emits.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_MINUTE_MS: u64 = 60_000;

    #[test]
    fn burst_beyond_window_budget_is_throttled() {
        // Three requotes per minute, no min-interval: a burst of five distinct
        // ticks inside the window grants only the budget of three.
        let mut budget = RequoteBudget::new(3, ONE_MINUTE_MS, 0);
        let granted = (0..5_u64)
            .filter(|&i| budget.try_acquire(1_000 + i * 100))
            .count();
        assert_eq!(granted, 3, "only the per-window budget is granted");
        assert_eq!(budget.in_window(), 3);
    }

    #[test]
    fn min_interval_throttles_back_to_back_requotes() {
        // Large window budget, 500ms minimum interval.
        let mut budget = RequoteBudget::new(100, ONE_MINUTE_MS, 500);
        assert!(budget.try_acquire(1_000));
        assert!(
            !budget.try_acquire(1_400),
            "400ms < 500ms min interval must throttle"
        );
        assert!(
            budget.try_acquire(1_500),
            "exactly the min interval is allowed"
        );
    }

    #[test]
    fn tokens_replenish_as_the_window_slides() {
        let mut budget = RequoteBudget::new(2, ONE_MINUTE_MS, 0);
        assert!(budget.try_acquire(1_000));
        assert!(budget.try_acquire(2_000));
        assert!(
            !budget.try_acquire(3_000),
            "the two-per-window budget is exhausted"
        );
        // Advance past the window so the 1_000ms command ages out, freeing a slot.
        assert!(
            budget.try_acquire(1_000 + ONE_MINUTE_MS + 1),
            "a slot frees once the oldest command leaves the window"
        );
    }

    #[test]
    fn zero_budget_permits_nothing() {
        let mut budget = RequoteBudget::new(0, ONE_MINUTE_MS, 0);
        assert!(!budget.try_acquire(1_000));
        assert!(!budget.try_acquire(2_000));
        assert_eq!(budget.in_window(), 0);
    }
}
