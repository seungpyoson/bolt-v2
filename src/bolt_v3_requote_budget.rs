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
    /// minimum interval. The config bridge
    /// `bolt_v3_maker_rate_budget::build_requote_budget_pair` derives these values
    /// from TOML and venue capability facts; this constructor only accounts for
    /// already-resolved values, so it is `pub(crate)` and reached only through that
    /// bridge in production (enforced by `verify_bolt_v3_requote_construction.py`).
    pub(crate) fn new(max_cost_per_window: u64, window_ms: u64, min_interval_ms: u64) -> Self {
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

        // `min_interval_ms` is the floor between DISTINCT requote ticks. Emits that
        // share the caller's `now_ms` belong to one quote cycle — e.g. both legs of a
        // binary market co-quoted from a single event drive this budget at the same
        // clock — and must not throttle each other; the sliding-window cap still
        // bounds same-tick bursts. Only a strictly-later tick inside the interval is
        // throttled (the `now_ms < last_ms` out-of-order case is already rejected
        // above, so here `now_ms >= last_ms`).
        if let Some(last_ms) = self.last_emit_ms
            && now_ms > last_ms
            && now_ms - last_ms < self.min_interval_ms
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

/// Structural token cost of one NT submit command against the submit-governor
/// budget. A submit (fresh or resubmit) issues exactly one submit command.
const SUBMIT_COMMAND_COST: u64 = 1;
/// Structural token cost of one venue REST call against the CLOB REST budget.
/// Every cancel and every submit is exactly one REST call.
const REST_CALL_COST: u64 = 1;
/// REST cost of a cancel+resubmit reprice cycle: the cancel REST call plus the
/// resubmit REST call. The venue lacks an in-place modify, so a reprice is always
/// two REST calls.
pub(crate) const CANCEL_RESUBMIT_REST_COST: u64 = REST_CALL_COST + REST_CALL_COST;

/// The two-budget maker requote admission gate (§16#3, FR-011).
///
/// A maker reprice is bounded by TWO independent constraints in DIFFERENT units,
/// which must not be collapsed into a single "whichever is lower" window:
/// - `submit_commands` — the NT submit-governor budget (`max_order_submit_rate`,
///   e.g. 40/min). Counts submit COMMANDS; a cancel is not a submit command.
/// - `rest_calls` — the venue CLOB REST budget
///   (`VenueEgressModel::cap_per_minute`, the canonical venue egress-capability
///   fact the config validator reconciles order rates against; e.g. 100/min).
///   Counts REST CALLS; every cancel and every submit is one REST call.
///
/// A cancel+resubmit reprice costs **1 submit command + 2 REST calls**. The gate
/// reserves both budgets for the WHOLE pair **atomically as one acquisition**
/// before the cancel is issued: if either budget would be exhausted, neither is
/// charged, so mid-window exhaustion can never strand a cancelled side with no
/// budget left to resubmit. Atomicity is enforced by trying both reservations on
/// throwaway clones and committing only when both succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequoteBudgetPair {
    submit_commands: RequoteBudget,
    rest_calls: RequoteBudget,
}

impl RequoteBudgetPair {
    /// Compose the submit-command and REST-call budgets. The config bridge
    /// `bolt_v3_maker_rate_budget::build_requote_budget_pair` derives each budget's
    /// caps/windows from TOML (`max_order_submit_rate`) and the venue egress
    /// capability fact (`VenueEgressModel::cap_per_minute`); this gate only composes
    /// them, so it is `pub(crate)` and reached only through that bridge in production
    /// (enforced by `verify_bolt_v3_requote_construction.py`).
    pub(crate) fn new(submit_commands: RequoteBudget, rest_calls: RequoteBudget) -> Self {
        Self {
            submit_commands,
            rest_calls,
        }
    }

    /// Reserve budget for a fresh submit (a leg with no resting order): one submit
    /// command and one REST call. All-or-nothing across both budgets.
    pub fn try_reserve_fresh_submit(&mut self, now_ms: u64) -> bool {
        self.try_reserve(now_ms, SUBMIT_COMMAND_COST, REST_CALL_COST)
    }

    /// Reserve budget for a cancel+resubmit reprice as ONE acquisition: one submit
    /// command and two REST calls. All-or-nothing across both budgets, so a
    /// granted cancel is always paired with a guaranteed resubmit token.
    pub fn try_reserve_cancel_resubmit(&mut self, now_ms: u64) -> bool {
        self.try_reserve(now_ms, SUBMIT_COMMAND_COST, CANCEL_RESUBMIT_REST_COST)
    }

    /// Reserve budget for a standalone cancel (no resubmit): zero submit commands
    /// and one REST call.
    pub fn try_reserve_cancel(&mut self, now_ms: u64) -> bool {
        self.try_reserve(now_ms, 0, REST_CALL_COST)
    }

    /// Granted submit commands currently counted inside the submit-governor window.
    pub fn submit_commands_in_window(&self) -> usize {
        self.submit_commands.in_window()
    }

    /// Total REST-call cost currently counted inside the venue REST window.
    pub fn rest_cost_in_window(&self) -> u64 {
        self.rest_calls.cost_in_window()
    }

    /// Atomically reserve `submit_cost` submit commands and `rest_cost` REST calls.
    /// Both reservations are attempted on throwaway clones; the live budgets are
    /// replaced only when BOTH succeed, so a failed reservation never leaves a
    /// partial charge on either budget. A zero submit cost (a standalone cancel)
    /// leaves the submit budget untouched.
    fn try_reserve(&mut self, now_ms: u64, submit_cost: u64, rest_cost: u64) -> bool {
        let mut submit_trial = self.submit_commands.clone();
        let mut rest_trial = self.rest_calls.clone();

        let submit_ok = submit_cost == 0 || submit_trial.try_acquire(now_ms, submit_cost);
        let rest_ok = rest_trial.try_acquire(now_ms, rest_cost);
        if !(submit_ok && rest_ok) {
            return false;
        }

        self.submit_commands = submit_trial;
        self.rest_calls = rest_trial;
        true
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
    fn co_incident_emits_at_the_same_tick_bypass_the_min_interval() {
        // Two acquisitions at the SAME now_ms belong to one quote cycle (e.g. both
        // legs of a binary market driven from a single event). With a 500ms interval
        // the second same-tick acquire MUST still be granted; only a strictly-later
        // tick inside the interval is throttled. A non-exempting gate would compute
        // 1_000 - 1_000 = 0 < 500 and wrongly refuse the second leg, so this asserts
        // the cross-leg fix is load-bearing.
        let mut budget = RequoteBudget::new(100, ONE_MINUTE_MS, 500);

        assert!(budget.try_acquire(1_000, 1));
        assert!(budget.try_acquire(1_000, 1), "same-tick co-quote must pass");
        assert_eq!(budget.in_window(), 2);
        // A distinct tick inside the interval is still throttled...
        assert!(!budget.try_acquire(1_100, 1));
        // ...and a tick at the interval boundary is admitted again.
        assert!(budget.try_acquire(1_500, 1));
    }

    #[test]
    fn same_tick_bursts_are_still_bounded_by_the_window_cap() {
        // The same-tick exemption skips the min-interval throttle but NOT the window
        // cap. With cap 2 and a 500ms interval, two co-incident acquires at one tick
        // are admitted (the exemption lets the second through), but a THIRD at the same
        // tick must be refused by the capacity guard (next_cost 3 > cap 2), never by
        // the interval. This pins that the exemption cannot be abused to burst past the
        // rate cap inside a single millisecond. The second acquire also fails against
        // the pre-fix saturating_sub gate (0 < 500), so this is load-bearing too.
        let mut budget = RequoteBudget::new(2, ONE_MINUTE_MS, 500);

        assert!(budget.try_acquire(1_000, 1));
        assert!(
            budget.try_acquire(1_000, 1),
            "second same-tick emit fits under the cap"
        );
        assert!(
            !budget.try_acquire(1_000, 1),
            "third same-tick emit must be refused by the window cap, not the interval"
        );
        assert_eq!(budget.in_window(), 2);
        assert_eq!(budget.cost_in_window(), 2);
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
    fn timestamp_exactly_at_eviction_cutoff_is_retained() {
        // Pins the INCLUSIVE trailing edge of the eviction loop: an emit whose
        // timestamp equals `now - window_ms` is still INSIDE the window and must be
        // RETAINED, not evicted. The sibling test above only exercises the
        // `now <= window_ms` early-return and the strictly-below-cutoff case (front
        // at 0 vs cutoff 1), so it never places a front entry exactly at the cutoff
        // while the eviction loop runs. Here the first emit lands at 1_000 and the
        // second acquisition is exactly one window later, so `cutoff == 1_000 ==`
        // the front emit's timestamp.
        let mut budget = RequoteBudget::new(1, ONE_MINUTE_MS, 0);

        assert!(budget.try_acquire(1_000, 1));
        // With the inclusive `timestamp_ms >= cutoff_ms` keep-condition the front is
        // retained, exhausting the cap-1 budget, so the boundary reprice is DENIED.
        // Weakening evict's compare to `>` (which would evict the exactly-at-cutoff
        // emit) frees the budget and GRANTS this acquisition, flipping the assert —
        // so this test fails on that mutant.
        assert!(!budget.try_acquire(1_000 + ONE_MINUTE_MS, 1));
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

    fn fresh_pair(submit_cap: u64, rest_cap: u64) -> RequoteBudgetPair {
        RequoteBudgetPair::new(
            RequoteBudget::new(submit_cap, ONE_MINUTE_MS, 0),
            RequoteBudget::new(rest_cap, ONE_MINUTE_MS, 0),
        )
    }

    #[test]
    fn fresh_submit_charges_one_submit_command_and_one_rest_call() {
        let mut pair = fresh_pair(40, 100);
        assert!(pair.try_reserve_fresh_submit(1_000));
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 1);
    }

    #[test]
    fn cancel_resubmit_charges_one_submit_command_and_two_rest_calls() {
        let mut pair = fresh_pair(40, 100);
        assert!(pair.try_reserve_cancel_resubmit(1_000));
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 2);
    }

    #[test]
    fn standalone_cancel_charges_one_rest_call_and_zero_submit_commands() {
        let mut pair = fresh_pair(40, 100);
        assert!(pair.try_reserve_cancel(1_000));
        assert_eq!(pair.submit_commands_in_window(), 0);
        assert_eq!(pair.rest_cost_in_window(), 1);
    }

    #[test]
    fn the_two_budgets_are_independent_constraints_submit_can_bind_first() {
        // Submit-governor caps at 2 while REST has ample room; the third fresh
        // submit must fail on the SUBMIT budget, proving the constraints are NOT
        // collapsed into a single "whichever is lower" window.
        let mut pair = fresh_pair(2, 100);
        assert!(pair.try_reserve_fresh_submit(1_000));
        assert!(pair.try_reserve_fresh_submit(1_100));
        assert!(!pair.try_reserve_fresh_submit(1_200));
        assert_eq!(pair.submit_commands_in_window(), 2);
        // REST charged exactly twice (the two granted submits), not three times.
        assert_eq!(pair.rest_cost_in_window(), 2);
    }

    #[test]
    fn the_two_budgets_are_independent_constraints_rest_can_bind_first() {
        // REST caps at 3 while the submit-governor has ample room; a cancel+resubmit
        // costs 2 REST, so the first fits but the second (needing 2 more, total 4)
        // must fail on the REST budget.
        let mut pair = fresh_pair(100, 3);
        assert!(pair.try_reserve_cancel_resubmit(1_000));
        assert!(!pair.try_reserve_cancel_resubmit(1_100));
        // Only the first cancel+resubmit landed: 1 submit command, 2 REST calls.
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 2);
    }

    #[test]
    fn failed_rest_reservation_leaves_the_submit_budget_uncharged() {
        // Anti-stranding atomicity: a cancel+resubmit needs 2 REST but only 1 fits.
        // A non-atomic gate would charge the submit command first, then fail on REST,
        // stranding a submit token. The atomic gate must charge NEITHER budget.
        let mut pair = fresh_pair(40, 1);
        assert!(!pair.try_reserve_cancel_resubmit(1_000));
        assert_eq!(
            pair.submit_commands_in_window(),
            0,
            "submit budget must be untouched"
        );
        assert_eq!(
            pair.rest_cost_in_window(),
            0,
            "rest budget must be untouched"
        );
        // The gate is not poisoned: a later affordable standalone cancel still works.
        assert!(pair.try_reserve_cancel(1_100));
        assert_eq!(pair.rest_cost_in_window(), 1);
    }

    #[test]
    fn failed_submit_reservation_leaves_the_rest_budget_uncharged() {
        // The mirror case: the submit-governor is exhausted but REST has room. The
        // cancel+resubmit must charge NEITHER budget (no partial 2-REST charge).
        let mut pair = fresh_pair(1, 100);
        assert!(pair.try_reserve_fresh_submit(1_000)); // exhausts the 1-command submit budget
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 1);
        assert!(!pair.try_reserve_cancel_resubmit(1_100));
        // The failed reprice added neither a submit command nor any REST cost.
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 1);
    }

    #[test]
    fn budgets_replenish_as_both_windows_slide() {
        let mut pair = fresh_pair(1, 2);
        assert!(pair.try_reserve_cancel_resubmit(1_000)); // 1 submit, 2 rest -> both full
        assert!(!pair.try_reserve_cancel_resubmit(1_100));
        // After both one-minute windows slide past, a fresh reprice is admitted again.
        assert!(pair.try_reserve_cancel_resubmit(1_001 + ONE_MINUTE_MS));
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 2);
    }

    #[test]
    fn two_granted_cancel_resubmits_drive_the_budgets_to_different_levels() {
        // The decisive independence check: this is the ONLY pair test where both
        // budgets bind at once AND both are GRANTED. With submit cap 2 and REST cap
        // 100, two cancel+resubmits cost 2 submit commands but 4 REST calls, so the
        // two windows MUST reach DIFFERENT fill levels (2 vs 4) while both succeed.
        // No single collapsed "whichever is lower" window can reproduce this: a
        // min-cap-2 window charging 2 per reprice would reject the second reprice,
        // failing the grant assertion below. The grant/deny tests above max out one
        // budget to force a DENY; only this test pins both-bind-and-both-grant.
        let mut pair = fresh_pair(2, 100);
        assert!(pair.try_reserve_cancel_resubmit(1_000));
        assert!(pair.try_reserve_cancel_resubmit(1_100));
        assert_eq!(pair.submit_commands_in_window(), 2);
        assert_eq!(pair.rest_cost_in_window(), 4);
    }

    #[test]
    fn a_failed_reservation_does_not_poison_the_submit_min_interval() {
        // last_emit_ms atomicity. The submit budget carries a 500ms minimum spacing.
        // A cancel+resubmit needs 2 REST but REST caps at 1, so the reservation
        // fails and commits NOTHING. A non-atomic gate that touched the live submit
        // budget's last_emit_ms before the REST check failed — then rolled back only
        // the cost, not the timestamp — would wrongly throttle the very next submit.
        // The atomic clone-and-commit gate leaves the submit budget pristine
        // (last_emit_ms still None), so a fresh submit only 100ms later (well inside
        // the 500ms interval) is still admitted. A poisoned-timestamp variant would
        // see 1_100 - 1_000 = 100 < 500 and return false here.
        let mut pair = RequoteBudgetPair::new(
            RequoteBudget::new(40, ONE_MINUTE_MS, 500),
            RequoteBudget::new(1, ONE_MINUTE_MS, 500),
        );
        assert!(!pair.try_reserve_cancel_resubmit(1_000));
        assert!(pair.try_reserve_fresh_submit(1_100));
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 1);
    }

    #[test]
    fn a_standalone_cancels_rest_call_throttles_a_later_submit_on_the_rest_floor_only() {
        // The two budgets track their OWN last-activity independently — that is the
        // point of separating them, not a desync bug. A standalone cancel consumes a
        // REST call but no submit command, so it advances ONLY the REST budget's
        // min-interval floor. A fresh submit a short tick later is therefore refused
        // on the REST floor (the cancel used the venue's REST channel too recently)
        // while the submit-governor floor stays pristine. Both budgets carry a 500ms
        // interval; the cancel at 1_000 leaves submit untouched, so the 1_100 submit
        // is blocked by REST, and once the REST interval clears the submit lands.
        let mut pair = RequoteBudgetPair::new(
            RequoteBudget::new(40, ONE_MINUTE_MS, 500),
            RequoteBudget::new(100, ONE_MINUTE_MS, 500),
        );
        assert!(pair.try_reserve_cancel(1_000));
        assert_eq!(pair.submit_commands_in_window(), 0);
        assert_eq!(pair.rest_cost_in_window(), 1);
        // Blocked on the REST floor (1_100 - 1_000 = 100 < 500); neither budget moves.
        assert!(!pair.try_reserve_fresh_submit(1_100));
        assert_eq!(pair.submit_commands_in_window(), 0);
        assert_eq!(pair.rest_cost_in_window(), 1);
        // Once the REST interval clears, the submit lands and charges both budgets.
        assert!(pair.try_reserve_fresh_submit(1_500));
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 2);
    }

    #[test]
    fn asymmetric_min_intervals_still_exempt_co_incident_reservations() {
        // RequoteBudgetPair composes two budgets that each carry their OWN interval.
        // A future config may derive the submit floor (from max_order_submit_rate) and
        // the REST floor (from clob_per_minute) independently, so the two intervals can
        // differ. The same-tick exemption is per-budget and interval-value-agnostic:
        // both co-incident sub-reservations are exempt regardless of which interval is
        // longer. Two fresh submits at one tick must both land; a strictly-later tick
        // inside the longer (submit) interval is still throttled, and the atomic gate
        // leaves neither budget advanced when it refuses.
        let mut pair = RequoteBudgetPair::new(
            RequoteBudget::new(40, ONE_MINUTE_MS, 500),
            RequoteBudget::new(100, ONE_MINUTE_MS, 250),
        );
        assert!(pair.try_reserve_fresh_submit(1_000));
        assert!(
            pair.try_reserve_fresh_submit(1_000),
            "co-incident reservation must pass under asymmetric intervals"
        );
        assert_eq!(pair.submit_commands_in_window(), 2);
        assert_eq!(pair.rest_cost_in_window(), 2);
        // 1_100 is inside BOTH intervals (100 < 250 < 500): refused on the submit floor,
        // and being atomic it leaves both budgets exactly where they were.
        assert!(!pair.try_reserve_fresh_submit(1_100));
        assert_eq!(pair.submit_commands_in_window(), 2);
        assert_eq!(pair.rest_cost_in_window(), 2);
    }
}
