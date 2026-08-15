//! Shared maker requote-rate throttle.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex, MutexGuard},
};

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
    /// bridge in production by convention. No static fence currently prevents
    /// other same-crate code from calling this `pub(crate)` constructor directly.
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

    pub fn max_cost_per_window(&self) -> u64 {
        self.max_cost_per_window
    }

    pub fn window_ms(&self) -> u64 {
        self.window_ms
    }

    pub fn min_interval_ms(&self) -> u64 {
        self.min_interval_ms
    }

    pub fn last_emit_ms(&self) -> Option<u64> {
        self.last_emit_ms
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

    fn record_reserved(&mut self, now_ms: u64, cost: u64) {
        let emit_ms = self.last_emit_ms.map_or(now_ms, |last| last.max(now_ms));
        self.evict(emit_ms);
        self.emits.push_back((emit_ms, cost));
        self.window_cost = self
            .window_cost
            .checked_add(cost)
            .expect("reserved budget conversion cannot overflow");
        self.last_emit_ms = Some(emit_ms);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequoteBudgetReservationKind {
    FreshSubmit,
    CancelResubmit,
    Rest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequoteBudgetReservationProposal {
    now_ms: u64,
    submit_cost: u64,
    rest_cost: u64,
    kind: RequoteBudgetReservationKind,
}

impl RequoteBudgetReservationProposal {
    #[must_use]
    pub const fn kind(self) -> RequoteBudgetReservationKind {
        self.kind
    }

    #[must_use]
    pub const fn now_ms(self) -> u64 {
        self.now_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequoteBudgetReservationDenied {
    Capacity,
    GenerationOverflow,
    StaleReservation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OutstandingLiability {
    now_ms: u64,
    submit_cost: u64,
    rest_cost: u64,
    kind: RequoteBudgetReservationKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequoteBudgetPairState {
    submit_commands: RequoteBudget,
    rest_calls: RequoteBudget,
    next_reservation_generation: u64,
    outstanding: BTreeMap<u64, OutstandingLiability>,
}

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
#[derive(Clone)]
pub struct RequoteBudgetPair {
    state: Arc<Mutex<RequoteBudgetPairState>>,
}

impl std::fmt::Debug for RequoteBudgetPair {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.lock().fmt(formatter)
    }
}

impl PartialEq for RequoteBudgetPair {
    fn eq(&self, other: &Self) -> bool {
        if Arc::ptr_eq(&self.state, &other.state) {
            return true;
        }
        let left = self.lock().clone();
        let right = other.lock().clone();
        left == right
    }
}

impl Eq for RequoteBudgetPair {}

#[derive(Debug)]
pub struct RequoteBudgetReservation {
    budget: RequoteBudgetPair,
    generation: u64,
    state: RequoteBudgetReservationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequoteBudgetReservationState {
    Reserved,
    ReplacementReserved,
    SinkInvoked,
    Settled,
}

impl RequoteBudgetPair {
    /// Compose the submit-command and REST-call budgets. The config bridge
    /// `bolt_v3_maker_rate_budget::build_requote_budget_pair` derives each budget's
    /// caps/windows from TOML (`max_order_submit_rate`) and the venue egress
    /// capability fact (`VenueEgressModel::cap_per_minute`); this gate only composes
    /// them, so it is `pub(crate)` and reached only through that bridge in production
    /// by convention. No static fence currently prevents other same-crate code from
    /// calling this `pub(crate)` constructor directly.
    pub(crate) fn new(submit_commands: RequoteBudget, rest_calls: RequoteBudget) -> Self {
        Self {
            state: Arc::new(Mutex::new(RequoteBudgetPairState {
                submit_commands,
                rest_calls,
                next_reservation_generation: 0,
                outstanding: BTreeMap::new(),
            })),
        }
    }

    pub fn propose_fresh_submit(
        &self,
        now_ms: u64,
    ) -> Result<RequoteBudgetReservationProposal, RequoteBudgetReservationDenied> {
        self.propose(
            now_ms,
            SUBMIT_COMMAND_COST,
            REST_CALL_COST,
            RequoteBudgetReservationKind::FreshSubmit,
        )
    }

    pub fn propose_cancel_resubmit(
        &self,
        now_ms: u64,
    ) -> Result<RequoteBudgetReservationProposal, RequoteBudgetReservationDenied> {
        self.propose(
            now_ms,
            SUBMIT_COMMAND_COST,
            CANCEL_RESUBMIT_REST_COST,
            RequoteBudgetReservationKind::CancelResubmit,
        )
    }

    pub fn propose_rest(
        &self,
        now_ms: u64,
    ) -> Result<RequoteBudgetReservationProposal, RequoteBudgetReservationDenied> {
        self.propose(
            now_ms,
            0,
            REST_CALL_COST,
            RequoteBudgetReservationKind::Rest,
        )
    }

    pub fn reserve(
        &self,
        proposal: RequoteBudgetReservationProposal,
    ) -> Result<RequoteBudgetReservation, RequoteBudgetReservationDenied> {
        let mut state = self.lock();
        if !Self::has_capacity(
            &state,
            proposal.now_ms,
            proposal.submit_cost,
            proposal.rest_cost,
        ) {
            return Err(RequoteBudgetReservationDenied::Capacity);
        }
        let generation = state
            .next_reservation_generation
            .checked_add(1)
            .ok_or(RequoteBudgetReservationDenied::GenerationOverflow)?;
        state.next_reservation_generation = generation;
        state.outstanding.insert(
            generation,
            OutstandingLiability {
                now_ms: proposal.now_ms,
                submit_cost: proposal.submit_cost,
                rest_cost: proposal.rest_cost,
                kind: proposal.kind,
            },
        );
        Ok(RequoteBudgetReservation {
            budget: self.clone(),
            generation,
            state: RequoteBudgetReservationState::Reserved,
        })
    }

    /// Granted submit commands currently counted inside the submit-governor window.
    pub fn submit_commands_in_window(&self) -> usize {
        self.lock().submit_commands.in_window()
    }

    /// Total REST-call cost currently counted inside the venue REST window.
    pub fn rest_cost_in_window(&self) -> u64 {
        self.lock().rest_calls.cost_in_window()
    }

    pub fn outstanding_submit_cost(&self) -> u64 {
        self.lock()
            .outstanding
            .values()
            .map(|liability| liability.submit_cost)
            .sum()
    }

    pub fn outstanding_rest_cost(&self) -> u64 {
        self.lock()
            .outstanding
            .values()
            .map(|liability| liability.rest_cost)
            .sum()
    }

    pub fn submit_command_cap(&self) -> u64 {
        self.lock().submit_commands.max_cost_per_window()
    }

    pub fn submit_window_ms(&self) -> u64 {
        self.lock().submit_commands.window_ms()
    }

    pub fn rest_cap_per_window(&self) -> u64 {
        self.lock().rest_calls.max_cost_per_window()
    }

    pub fn rest_window_ms(&self) -> u64 {
        self.lock().rest_calls.window_ms()
    }

    pub fn min_interval_ms(&self) -> u64 {
        let state = self.lock();
        state
            .submit_commands
            .min_interval_ms()
            .max(state.rest_calls.min_interval_ms())
    }

    pub fn last_emit_ms(&self) -> Option<u64> {
        let state = self.lock();
        state
            .submit_commands
            .last_emit_ms()
            .max(state.rest_calls.last_emit_ms())
    }

    fn propose(
        &self,
        now_ms: u64,
        submit_cost: u64,
        rest_cost: u64,
        kind: RequoteBudgetReservationKind,
    ) -> Result<RequoteBudgetReservationProposal, RequoteBudgetReservationDenied> {
        let state = self.lock();
        if !Self::has_capacity(&state, now_ms, submit_cost, rest_cost) {
            return Err(RequoteBudgetReservationDenied::Capacity);
        }
        Ok(RequoteBudgetReservationProposal {
            now_ms,
            submit_cost,
            rest_cost,
            kind,
        })
    }

    fn has_capacity(
        state: &RequoteBudgetPairState,
        now_ms: u64,
        submit_cost: u64,
        rest_cost: u64,
    ) -> bool {
        let outstanding_submit = state
            .outstanding
            .values()
            .try_fold(0_u64, |total, liability| {
                total.checked_add(liability.submit_cost)
            });
        let outstanding_rest = state
            .outstanding
            .values()
            .try_fold(0_u64, |total, liability| {
                total.checked_add(liability.rest_cost)
            });
        let (Some(outstanding_submit), Some(outstanding_rest)) =
            (outstanding_submit, outstanding_rest)
        else {
            return false;
        };
        let Some(required_submit) = outstanding_submit.checked_add(submit_cost) else {
            return false;
        };
        let Some(required_rest) = outstanding_rest.checked_add(rest_cost) else {
            return false;
        };
        let mut submit_trial = state.submit_commands.clone();
        let mut rest_trial = state.rest_calls.clone();
        let submit_ok = required_submit == 0 || submit_trial.try_acquire(now_ms, required_submit);
        let rest_ok = rest_trial.try_acquire(now_ms, required_rest);
        submit_ok && rest_ok
    }

    fn lock(&self) -> MutexGuard<'_, RequoteBudgetPairState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl RequoteBudgetReservation {
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn mark_sink_invoked_at(
        &mut self,
        now_ms: u64,
    ) -> Result<(), RequoteBudgetReservationDenied> {
        match self.state {
            RequoteBudgetReservationState::Reserved => {
                let mut state = self.budget.lock();
                let liability = state
                    .outstanding
                    .get(&self.generation)
                    .copied()
                    .ok_or(RequoteBudgetReservationDenied::StaleReservation)?;
                if liability.kind == RequoteBudgetReservationKind::CancelResubmit {
                    let replacement_rest_cost = liability
                        .rest_cost
                        .checked_sub(REST_CALL_COST)
                        .ok_or(RequoteBudgetReservationDenied::StaleReservation)?;
                    let replacement = state
                        .outstanding
                        .get_mut(&self.generation)
                        .ok_or(RequoteBudgetReservationDenied::StaleReservation)?;
                    replacement.now_ms = now_ms;
                    replacement.rest_cost = replacement_rest_cost;
                    state.rest_calls.record_reserved(now_ms, REST_CALL_COST);
                    self.state = RequoteBudgetReservationState::ReplacementReserved;
                    return Ok(());
                }
                Self::record_remaining_liability(&mut state, self.generation, now_ms)?;
                self.state = RequoteBudgetReservationState::SinkInvoked;
                Ok(())
            }
            RequoteBudgetReservationState::ReplacementReserved => {
                let mut state = self.budget.lock();
                Self::record_remaining_liability(&mut state, self.generation, now_ms)?;
                self.state = RequoteBudgetReservationState::SinkInvoked;
                Ok(())
            }
            RequoteBudgetReservationState::SinkInvoked => Ok(()),
            RequoteBudgetReservationState::Settled => {
                Err(RequoteBudgetReservationDenied::StaleReservation)
            }
        }
    }

    pub fn commit(mut self) -> Result<(), RequoteBudgetReservationDenied> {
        self.settle_committed()
    }

    pub fn abort(mut self) -> Result<(), RequoteBudgetReservationDenied> {
        self.settle_aborted()
    }

    fn settle_committed(&mut self) -> Result<(), RequoteBudgetReservationDenied> {
        let mut state = self.budget.lock();
        match self.state {
            RequoteBudgetReservationState::Reserved
            | RequoteBudgetReservationState::ReplacementReserved => {
                let now_ms = state
                    .outstanding
                    .get(&self.generation)
                    .map(|liability| liability.now_ms)
                    .ok_or(RequoteBudgetReservationDenied::StaleReservation)?;
                Self::record_remaining_liability(&mut state, self.generation, now_ms)?;
            }
            RequoteBudgetReservationState::SinkInvoked => {}
            RequoteBudgetReservationState::Settled => {
                return Err(RequoteBudgetReservationDenied::StaleReservation);
            }
        }
        self.state = RequoteBudgetReservationState::Settled;
        Ok(())
    }

    fn settle_aborted(&mut self) -> Result<(), RequoteBudgetReservationDenied> {
        let removed = self.budget.lock().outstanding.remove(&self.generation);
        if removed.is_none() {
            return Err(RequoteBudgetReservationDenied::StaleReservation);
        }
        self.state = RequoteBudgetReservationState::Settled;
        Ok(())
    }

    fn record_remaining_liability(
        state: &mut RequoteBudgetPairState,
        generation: u64,
        now_ms: u64,
    ) -> Result<(), RequoteBudgetReservationDenied> {
        let liability = state
            .outstanding
            .remove(&generation)
            .ok_or(RequoteBudgetReservationDenied::StaleReservation)?;
        if liability.submit_cost > 0 {
            state
                .submit_commands
                .record_reserved(now_ms, liability.submit_cost);
        }
        state
            .rest_calls
            .record_reserved(now_ms, liability.rest_cost);
        Ok(())
    }
}

impl Drop for RequoteBudgetReservation {
    fn drop(&mut self) {
        match self.state {
            RequoteBudgetReservationState::Reserved
            | RequoteBudgetReservationState::ReplacementReserved => {
                let _ = self.settle_aborted();
            }
            RequoteBudgetReservationState::SinkInvoked => {}
            RequoteBudgetReservationState::Settled => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_MINUTE_MS: u64 = 60_000;

    fn reserve_proposal(
        pair: &RequoteBudgetPair,
        proposal: Result<RequoteBudgetReservationProposal, RequoteBudgetReservationDenied>,
    ) -> bool {
        proposal
            .and_then(|proposal| pair.reserve(proposal))
            .and_then(RequoteBudgetReservation::commit)
            .is_ok()
    }

    fn reserve_fresh(pair: &RequoteBudgetPair, now_ms: u64) -> bool {
        reserve_proposal(pair, pair.propose_fresh_submit(now_ms))
    }

    fn reserve_cancel_resubmit(pair: &RequoteBudgetPair, now_ms: u64) -> bool {
        reserve_proposal(pair, pair.propose_cancel_resubmit(now_ms))
    }

    fn reserve_rest(pair: &RequoteBudgetPair, now_ms: u64) -> bool {
        reserve_proposal(pair, pair.propose_rest(now_ms))
    }

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
        let pair = fresh_pair(40, 100);
        assert!(reserve_fresh(&pair, 1_000));
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 1);
    }

    #[test]
    fn cancel_resubmit_charges_one_submit_command_and_two_rest_calls() {
        let pair = fresh_pair(40, 100);
        assert!(reserve_cancel_resubmit(&pair, 1_000));
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 2);
    }

    #[test]
    fn standalone_cancel_charges_one_rest_call_and_zero_submit_commands() {
        let pair = fresh_pair(40, 100);
        assert!(reserve_rest(&pair, 1_000));
        assert_eq!(pair.submit_commands_in_window(), 0);
        assert_eq!(pair.rest_cost_in_window(), 1);
    }

    #[test]
    fn the_two_budgets_are_independent_constraints_submit_can_bind_first() {
        // Submit-governor caps at 2 while REST has ample room; the third fresh
        // submit must fail on the SUBMIT budget, proving the constraints are NOT
        // collapsed into a single "whichever is lower" window.
        let pair = fresh_pair(2, 100);
        assert!(reserve_fresh(&pair, 1_000));
        assert!(reserve_fresh(&pair, 1_100));
        assert!(!reserve_fresh(&pair, 1_200));
        assert_eq!(pair.submit_commands_in_window(), 2);
        // REST charged exactly twice (the two granted submits), not three times.
        assert_eq!(pair.rest_cost_in_window(), 2);
    }

    #[test]
    fn the_two_budgets_are_independent_constraints_rest_can_bind_first() {
        // REST caps at 3 while the submit-governor has ample room; a cancel+resubmit
        // costs 2 REST, so the first fits but the second (needing 2 more, total 4)
        // must fail on the REST budget.
        let pair = fresh_pair(100, 3);
        assert!(reserve_cancel_resubmit(&pair, 1_000));
        assert!(!reserve_cancel_resubmit(&pair, 1_100));
        // Only the first cancel+resubmit landed: 1 submit command, 2 REST calls.
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 2);
    }

    #[test]
    fn failed_rest_reservation_leaves_the_submit_budget_uncharged() {
        // Anti-stranding atomicity: a cancel+resubmit needs 2 REST but only 1 fits.
        // A non-atomic gate would charge the submit command first, then fail on REST,
        // stranding a submit token. The atomic gate must charge NEITHER budget.
        let pair = fresh_pair(40, 1);
        assert!(!reserve_cancel_resubmit(&pair, 1_000));
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
        assert!(reserve_rest(&pair, 1_100));
        assert_eq!(pair.rest_cost_in_window(), 1);
    }

    #[test]
    fn failed_submit_reservation_leaves_the_rest_budget_uncharged() {
        // The mirror case: the submit-governor is exhausted but REST has room. The
        // cancel+resubmit must charge NEITHER budget (no partial 2-REST charge).
        let pair = fresh_pair(1, 100);
        assert!(reserve_fresh(&pair, 1_000)); // exhausts the 1-command submit budget
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 1);
        assert!(!reserve_cancel_resubmit(&pair, 1_100));
        // The failed reprice added neither a submit command nor any REST cost.
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 1);
    }

    #[test]
    fn budgets_replenish_as_both_windows_slide() {
        let pair = fresh_pair(1, 2);
        assert!(reserve_cancel_resubmit(&pair, 1_000)); // 1 submit, 2 rest -> both full
        assert!(!reserve_cancel_resubmit(&pair, 1_100));
        // After both one-minute windows slide past, a fresh reprice is admitted again.
        assert!(reserve_cancel_resubmit(&pair, 1_001 + ONE_MINUTE_MS));
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
        let pair = fresh_pair(2, 100);
        assert!(reserve_cancel_resubmit(&pair, 1_000));
        assert!(reserve_cancel_resubmit(&pair, 1_100));
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
        let pair = RequoteBudgetPair::new(
            RequoteBudget::new(40, ONE_MINUTE_MS, 500),
            RequoteBudget::new(1, ONE_MINUTE_MS, 500),
        );
        assert!(!reserve_cancel_resubmit(&pair, 1_000));
        assert!(reserve_fresh(&pair, 1_100));
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
        let pair = RequoteBudgetPair::new(
            RequoteBudget::new(40, ONE_MINUTE_MS, 500),
            RequoteBudget::new(100, ONE_MINUTE_MS, 500),
        );
        assert!(reserve_rest(&pair, 1_000));
        assert_eq!(pair.submit_commands_in_window(), 0);
        assert_eq!(pair.rest_cost_in_window(), 1);
        // Blocked on the REST floor (1_100 - 1_000 = 100 < 500); neither budget moves.
        assert!(!reserve_fresh(&pair, 1_100));
        assert_eq!(pair.submit_commands_in_window(), 0);
        assert_eq!(pair.rest_cost_in_window(), 1);
        // Once the REST interval clears, the submit lands and charges both budgets.
        assert!(reserve_fresh(&pair, 1_500));
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
        let pair = RequoteBudgetPair::new(
            RequoteBudget::new(40, ONE_MINUTE_MS, 500),
            RequoteBudget::new(100, ONE_MINUTE_MS, 250),
        );
        assert!(reserve_fresh(&pair, 1_000));
        assert!(
            reserve_fresh(&pair, 1_000),
            "co-incident reservation must pass under asymmetric intervals"
        );
        assert_eq!(pair.submit_commands_in_window(), 2);
        assert_eq!(pair.rest_cost_in_window(), 2);
        // 1_100 is inside BOTH intervals (100 < 250 < 500): refused on the submit floor,
        // and being atomic it leaves both budgets exactly where they were.
        assert!(!reserve_fresh(&pair, 1_100));
        assert_eq!(pair.submit_commands_in_window(), 2);
        assert_eq!(pair.rest_cost_in_window(), 2);
    }

    #[test]
    fn outstanding_liability_never_ages_out_and_denies_later_commands() {
        let pair = fresh_pair(1, 1);
        let proposal = pair
            .propose_fresh_submit(1_000)
            .expect("first proposal fits");
        let reservation = pair.reserve(proposal).expect("first reservation arms");

        assert_eq!(pair.submit_commands_in_window(), 0);
        assert_eq!(pair.rest_cost_in_window(), 0);
        assert_eq!(pair.outstanding_submit_cost(), 1);
        assert_eq!(pair.outstanding_rest_cost(), 1);
        assert_eq!(
            pair.propose_fresh_submit(1_001 + ONE_MINUTE_MS),
            Err(RequoteBudgetReservationDenied::Capacity),
            "an outstanding reservation remains capacity-bearing after its original window"
        );

        drop(reservation);
        assert!(pair.propose_fresh_submit(1_001 + ONE_MINUTE_MS).is_ok());
    }

    #[test]
    fn consumption_atomically_converts_liability_to_emitted_cost() {
        let pair = fresh_pair(1, 1);
        let proposal = pair.propose_fresh_submit(1_000).expect("proposal fits");
        let mut reservation = pair.reserve(proposal).expect("reservation arms");
        reservation
            .mark_sink_invoked_at(1_000)
            .expect("sink invocation records the emitted command");
        reservation
            .commit()
            .expect("unchanged configuration commits");

        assert_eq!(pair.outstanding_submit_cost(), 0);
        assert_eq!(pair.outstanding_rest_cost(), 0);
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 1);
    }

    #[test]
    fn delayed_consumption_retimestamps_the_prepaid_charge_without_a_capacity_recheck() {
        let pair = fresh_pair(2, 2);
        let mut prepaid = pair
            .reserve(
                pair.propose_fresh_submit(1_000)
                    .expect("prepaid proposal fits"),
            )
            .expect("prepaid reservation arms");
        assert!(reserve_fresh(&pair, 61_001));
        prepaid
            .mark_sink_invoked_at(61_001)
            .expect("delayed sink invocation records at actor time");
        prepaid
            .commit()
            .expect("liability conversion cannot fail on window capacity");

        assert_eq!(pair.outstanding_submit_cost(), 0);
        assert_eq!(pair.submit_commands_in_window(), 2);
        assert_eq!(pair.rest_cost_in_window(), 2);
        assert_eq!(
            pair.propose_fresh_submit(121_001),
            Err(RequoteBudgetReservationDenied::Capacity),
            "both delayed-consumption charges remain inside the inclusive trailing edge"
        );
        assert!(pair.propose_fresh_submit(121_002).is_ok());
    }

    #[test]
    fn dropping_before_sink_releases_the_exact_reservation_without_a_charge() {
        let pair = fresh_pair(1, 1);
        let reservation = pair
            .reserve(pair.propose_fresh_submit(1_000).expect("proposal fits"))
            .expect("reservation arms");
        let generation = reservation.generation();
        drop(reservation);

        assert_eq!(pair.outstanding_submit_cost(), 0);
        assert_eq!(pair.outstanding_rest_cost(), 0);
        assert_eq!(pair.submit_commands_in_window(), 0);
        assert_eq!(pair.rest_cost_in_window(), 0);
        let next = pair
            .reserve(
                pair.propose_fresh_submit(1_000)
                    .expect("released capacity is available"),
            )
            .expect("next reservation arms");
        assert!(next.generation() > generation);
    }
}
