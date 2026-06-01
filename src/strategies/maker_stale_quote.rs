//! Pure, NautilusTrader-free stale-resting-quote age alarm for the binary-oracle
//! maker (W6 — observability).
//!
//! The taker rests nothing, so it never needs this; the maker rests two-sided
//! quotes that drift away from fair value the longer they sit unrefreshed. The
//! lifecycle machine in [`crate::strategies::quote_lifecycle`] tracks a leg's
//! *liveness* (Idle / Resting / in-flight), not its *age* — a quote that has
//! rested untouched for minutes is still `Resting` there and looks healthy. This
//! module supplies the missing age dimension: given each leg's last-rest
//! timestamp and a maximum tolerated rest age, it reports which resting legs have
//! gone stale so the observability layer can alarm (and, via the optional
//! [`StaleQuoteVerdict::refresh_intent`] helper, name the lifecycle refresh the
//! stale leg should take).
//!
//! Like its sibling maker modules it holds no NautilusTrader type and reads no
//! clock: the current time is passed in as `now_ms` (NO CLOCK), so the alarm is a
//! pure function of (now, last-rest timestamps, max age) and is exhaustively
//! unit-testable without a runtime. It names only *intent* in the existing
//! [`crate::strategies::quote_lifecycle`] vocabulary ([`Leg`] /
//! [`LifecycleAction`] / [`MarketAction`]) — there is no parallel staleness enum
//! (NO DUAL PATHS).
//!
//! Fail-closed throughout, because a stale quote is an adverse-selection hazard
//! and a missed alarm is worse than a spurious one:
//! - An **out-of-order / clock-skewed** rest timestamp (`now_ms < last_rest_ms`)
//!   is treated as STALE. This codebase fails closed on out-of-order timestamps:
//!   a quote whose rest time is inconsistent with the current clock cannot be
//!   trusted to be fresh, so it is flagged rather than silently believed young.
//! - A `max_rest_age_ms` of zero tolerates **no** resting age at all, so every
//!   resting leg is immediately stale. Zero is the most conservative possible
//!   bound, and the natural fail-closed reading of a degenerate/unset threshold:
//!   it can only over-alarm, never under-alarm.
//! - A **not-resting** leg (`None` last-rest timestamp) is never stale: there is
//!   no resting quote to age, so there is nothing to alarm on. This is the one
//!   non-stale fail-closed outcome — flagging a leg that holds no order would be
//!   a false alarm, not a conservative one.

use crate::strategies::quote_lifecycle::{Leg, LifecycleAction, MarketAction};

/// Whether one leg's resting quote is fresh, stale, or simply not resting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegStaleness {
    /// The leg holds no resting quote (`None` last-rest timestamp) — there is no
    /// age to evaluate, so it can never be stale.
    NotResting,
    /// The leg rests a quote whose age is below `max_rest_age_ms` — still fresh.
    Fresh,
    /// The leg rests a quote that has aged to or beyond `max_rest_age_ms`, or
    /// whose rest timestamp is out-of-order with `now_ms` (clock skew). Either way
    /// the resting quote can no longer be trusted fresh and must be refreshed.
    Stale,
}

impl LegStaleness {
    /// Whether this verdict is the [`LegStaleness::Stale`] alarm state.
    pub fn is_stale(self) -> bool {
        matches!(self, LegStaleness::Stale)
    }
}

/// The staleness verdict for one quote leg: which side it is and its
/// [`LegStaleness`].
///
/// Reuses [`Leg`] for the side identity rather than inventing a parallel side
/// type, so a verdict drops straight into the same per-leg routing as every other
/// maker intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleQuoteVerdict {
    /// Which side of the binary market this verdict describes.
    pub leg: Leg,
    /// The staleness classification of that leg's resting quote.
    pub staleness: LegStaleness,
}

impl StaleQuoteVerdict {
    /// Classify one leg's resting quote against the maximum tolerated rest age.
    ///
    /// `last_rest_ms` is the timestamp (ms) at which this leg's quote last began
    /// resting, or `None` when the leg is not resting. The age is
    /// `now_ms - last_rest_ms`, and the leg is stale once that age is `>=
    /// max_rest_age_ms` (the threshold instant itself counts as stale: a quote
    /// exactly `max_rest_age_ms` old has reached the limit).
    ///
    /// Fail-closed branches (see the module docs):
    /// - `last_rest_ms == None` -> [`LegStaleness::NotResting`] (never stale).
    /// - `now_ms < last_rest_ms` (out-of-order / clock skew) -> [`LegStaleness::Stale`].
    /// - `max_rest_age_ms == 0` -> any resting leg is [`LegStaleness::Stale`]
    ///   (covered by the `>=` comparison: every non-negative age is `>= 0`).
    pub fn classify(
        leg: Leg,
        now_ms: u64,
        last_rest_ms: Option<u64>,
        max_rest_age_ms: u64,
    ) -> Self {
        let staleness = match last_rest_ms {
            // A leg that is not resting has no quote to age — never stale.
            None => LegStaleness::NotResting,
            Some(last_rest_ms) => {
                if now_ms < last_rest_ms {
                    // Out-of-order / clock-skewed rest timestamp: this codebase
                    // fails closed on out-of-order timestamps. A quote whose rest
                    // time is in the future of `now_ms` cannot be trusted fresh,
                    // so it is flagged stale rather than yielding a nonsensical
                    // (wrapping) age.
                    LegStaleness::Stale
                } else if now_ms - last_rest_ms >= max_rest_age_ms {
                    // Aged to or beyond the tolerated maximum. With
                    // `max_rest_age_ms == 0` this catches every resting age
                    // (all `>= 0`), the fail-closed zero-threshold behaviour.
                    LegStaleness::Stale
                } else {
                    LegStaleness::Fresh
                }
            }
        };
        Self { leg, staleness }
    }

    /// Whether this leg's verdict is the stale alarm state.
    pub fn is_stale(self) -> bool {
        self.staleness.is_stale()
    }

    /// Map a stale verdict to the lifecycle refresh intent the strategy shell
    /// should act on, in the existing [`MarketAction`] vocabulary.
    ///
    /// A stale resting quote must be pulled and re-quoted at fresh value, so the
    /// intent is a per-leg [`LifecycleAction::Cancel`] for this side — the same
    /// cancel the lifecycle machine emits on the cancel+resubmit requote path and
    /// on a one-sided wind-down. The strategy shell drives the actual cancel
    /// (and any replacement submit) through the leg's [`QuoteLeg`] so the
    /// lifecycle state stays the single owner of order transitions; this helper
    /// only names *which* leg to act on and *that* a cancel is warranted (NT-FIRST,
    /// NO DUAL PATHS). [`Cancel`] — not a bespoke "refresh" action — keeps the
    /// alarm inside the lifecycle's existing intent set rather than inventing a
    /// parallel one.
    ///
    /// Returns `None` for a [`LegStaleness::Fresh`] or [`LegStaleness::NotResting`]
    /// leg: a fresh quote needs no action, and a non-resting leg has no order to
    /// cancel.
    ///
    /// [`QuoteLeg`]: crate::strategies::quote_lifecycle::QuoteLeg
    /// [`Cancel`]: crate::strategies::quote_lifecycle::LifecycleAction::Cancel
    pub fn refresh_intent(self) -> Option<MarketAction> {
        self.is_stale().then_some(MarketAction::Leg {
            leg: self.leg,
            action: LifecycleAction::Cancel,
        })
    }
}

/// A market's two per-leg staleness verdicts (YES and NO) — the unit the alarm
/// reports.
///
/// Holding both legs explicitly (rather than a variable-length collection)
/// mirrors the fixed two-leg shape of `MarketQuote`/`MarketReconcileSnapshot` and
/// guarantees the alarm visits each side exactly once, in deterministic
/// YES-then-NO order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketStaleQuoteAlarm {
    yes: StaleQuoteVerdict,
    no: StaleQuoteVerdict,
}

impl MarketStaleQuoteAlarm {
    /// Evaluate the staleness alarm for both legs of a market as of `now_ms`.
    ///
    /// `yes_last_rest_ms` / `no_last_rest_ms` are each leg's last-rest timestamp
    /// (`None` when that side is not resting); `max_rest_age_ms` is the single
    /// tolerated rest age shared by both legs (the strategy layer resolves it from
    /// config — there is no hardcoded threshold here). Each leg is classified by
    /// [`StaleQuoteVerdict::classify`], so every fail-closed branch (out-of-order
    /// timestamp, zero threshold, not-resting) applies per leg.
    pub fn evaluate(
        now_ms: u64,
        yes_last_rest_ms: Option<u64>,
        no_last_rest_ms: Option<u64>,
        max_rest_age_ms: u64,
    ) -> Self {
        Self {
            yes: StaleQuoteVerdict::classify(Leg::Yes, now_ms, yes_last_rest_ms, max_rest_age_ms),
            no: StaleQuoteVerdict::classify(Leg::No, now_ms, no_last_rest_ms, max_rest_age_ms),
        }
    }

    /// The verdict for one leg.
    pub fn verdict(&self, leg: Leg) -> StaleQuoteVerdict {
        match leg {
            Leg::Yes => self.yes,
            Leg::No => self.no,
        }
    }

    /// Whether any leg is stale — the single boolean the observability layer
    /// alarms on.
    pub fn any_stale(&self) -> bool {
        self.yes.is_stale() || self.no.is_stale()
    }

    /// The verdicts for every stale leg, in deterministic YES-then-NO order.
    ///
    /// A fresh or not-resting leg is omitted, so the result is exactly the legs
    /// that tripped the alarm. The order is fixed (YES before NO) so the alarm
    /// output — and any downstream log line or refresh plan built from it — is
    /// deterministic.
    pub fn stale_legs(&self) -> Vec<StaleQuoteVerdict> {
        [self.yes, self.no]
            .into_iter()
            .filter(StaleQuoteVerdict::is_stale_ref)
            .collect()
    }

    /// The lifecycle refresh intents for every stale leg, in deterministic
    /// YES-then-NO order.
    ///
    /// Each entry is the per-leg [`LifecycleAction::Cancel`] from
    /// [`StaleQuoteVerdict::refresh_intent`]; fresh and not-resting legs emit
    /// nothing. A market with no stale legs yields an empty plan, so a caller can
    /// drive the refresh through the same `MarketAction` path it uses for every
    /// other lifecycle intent without special-casing the no-alarm path.
    pub fn refresh_plan(&self) -> Vec<MarketAction> {
        [self.yes, self.no]
            .into_iter()
            .filter_map(StaleQuoteVerdict::refresh_intent)
            .collect()
    }
}

impl StaleQuoteVerdict {
    /// `filter`-friendly borrow of [`Self::is_stale`] (which consumes `self`),
    /// so the iterator adapters above need no closure.
    fn is_stale_ref(verdict: &Self) -> bool {
        verdict.is_stale()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test-only fixtures. Literals here are stripped by the source-fence before
    // the no-bare-literals production check, so they are permitted in #[cfg(test)].
    const REST_TS_MS: u64 = 10_000;
    const MAX_AGE_MS: u64 = 5_000;

    // --- single-leg classification: the age axis ---

    #[test]
    fn fresh_below_threshold_is_not_stale() {
        // Age 4_999ms < 5_000ms threshold -> Fresh.
        let verdict = StaleQuoteVerdict::classify(
            Leg::Yes,
            REST_TS_MS + MAX_AGE_MS - 1,
            Some(REST_TS_MS),
            MAX_AGE_MS,
        );
        assert_eq!(verdict.staleness, LegStaleness::Fresh);
        assert!(!verdict.is_stale());
        assert_eq!(verdict.refresh_intent(), None);
    }

    #[test]
    fn exactly_at_threshold_is_stale() {
        // Age == max_rest_age_ms reaches the limit -> Stale (the `>=` boundary).
        let verdict = StaleQuoteVerdict::classify(
            Leg::Yes,
            REST_TS_MS + MAX_AGE_MS,
            Some(REST_TS_MS),
            MAX_AGE_MS,
        );
        assert_eq!(verdict.staleness, LegStaleness::Stale);
        assert!(verdict.is_stale());
    }

    #[test]
    fn over_threshold_is_stale() {
        let verdict = StaleQuoteVerdict::classify(
            Leg::Yes,
            REST_TS_MS + MAX_AGE_MS + 1,
            Some(REST_TS_MS),
            MAX_AGE_MS,
        );
        assert_eq!(verdict.staleness, LegStaleness::Stale);
    }

    // --- single-leg classification: the fail-closed branches ---

    #[test]
    fn out_of_order_timestamp_is_stale() {
        // now_ms < last_rest_ms (clock skew / out-of-order): an inconsistent rest
        // timestamp cannot be trusted fresh, so the leg fails closed to Stale —
        // never a wrapping/underflowing age.
        let verdict =
            StaleQuoteVerdict::classify(Leg::Yes, REST_TS_MS - 1, Some(REST_TS_MS), MAX_AGE_MS);
        assert_eq!(verdict.staleness, LegStaleness::Stale);
        // Even one millisecond before the rest time is out-of-order.
        let verdict =
            StaleQuoteVerdict::classify(Leg::No, REST_TS_MS - 1, Some(REST_TS_MS), MAX_AGE_MS);
        assert_eq!(verdict.staleness, LegStaleness::Stale);
    }

    #[test]
    fn now_equal_to_rest_timestamp_is_fresh_not_out_of_order() {
        // now_ms == last_rest_ms is age 0, the in-order boundary: not out-of-order,
        // and (with a positive threshold) Fresh.
        let verdict =
            StaleQuoteVerdict::classify(Leg::Yes, REST_TS_MS, Some(REST_TS_MS), MAX_AGE_MS);
        assert_eq!(verdict.staleness, LegStaleness::Fresh);
    }

    #[test]
    fn not_resting_leg_is_never_stale() {
        // A None last-rest timestamp means the leg holds no order: there is no age
        // to evaluate, so it is NotResting regardless of now_ms or the threshold.
        let verdict = StaleQuoteVerdict::classify(Leg::Yes, REST_TS_MS, None, MAX_AGE_MS);
        assert_eq!(verdict.staleness, LegStaleness::NotResting);
        assert!(!verdict.is_stale());
        assert_eq!(verdict.refresh_intent(), None);
        // ...even at now_ms == 0 with a zero threshold (no order to age or cancel).
        let verdict = StaleQuoteVerdict::classify(Leg::No, 0, None, 0);
        assert_eq!(verdict.staleness, LegStaleness::NotResting);
        assert_eq!(verdict.refresh_intent(), None);
    }

    #[test]
    fn zero_threshold_makes_any_resting_leg_stale() {
        // max_rest_age_ms == 0 tolerates no resting age: a quote that just rested
        // this instant (age 0) is already stale. Fail-closed: a zero/unset
        // threshold can only over-alarm.
        let verdict = StaleQuoteVerdict::classify(Leg::Yes, REST_TS_MS, Some(REST_TS_MS), 0);
        assert_eq!(verdict.staleness, LegStaleness::Stale);
        // A not-resting leg is still never stale, even with a zero threshold.
        let verdict = StaleQuoteVerdict::classify(Leg::No, REST_TS_MS, None, 0);
        assert_eq!(verdict.staleness, LegStaleness::NotResting);
    }

    // --- the refresh-intent helper ---

    #[test]
    fn stale_verdict_maps_to_a_per_leg_cancel_intent() {
        // A stale leg's refresh intent is a per-leg Cancel in the lifecycle
        // vocabulary — not a bespoke "refresh" action.
        for leg in [Leg::Yes, Leg::No] {
            let verdict = StaleQuoteVerdict::classify(
                leg,
                REST_TS_MS + MAX_AGE_MS,
                Some(REST_TS_MS),
                MAX_AGE_MS,
            );
            assert_eq!(
                verdict.refresh_intent(),
                Some(MarketAction::Leg {
                    leg,
                    action: LifecycleAction::Cancel,
                })
            );
        }
    }

    // --- both-leg market alarm ---

    #[test]
    fn market_evaluates_both_legs_independently() {
        // YES rests fresh, NO rests stale (older), evaluated against one now_ms.
        let now_ms = REST_TS_MS + MAX_AGE_MS + 1;
        let alarm = MarketStaleQuoteAlarm::evaluate(
            now_ms,
            Some(now_ms - 1), // YES rested 1ms ago -> Fresh
            Some(REST_TS_MS), // NO rested long ago -> Stale
            MAX_AGE_MS,
        );
        assert_eq!(alarm.verdict(Leg::Yes).staleness, LegStaleness::Fresh);
        assert_eq!(alarm.verdict(Leg::No).staleness, LegStaleness::Stale);
        assert!(alarm.any_stale());
    }

    #[test]
    fn market_with_no_stale_legs_alarms_nothing() {
        // Both legs fresh -> no alarm, empty stale-leg and refresh plans.
        let alarm = MarketStaleQuoteAlarm::evaluate(
            REST_TS_MS,
            Some(REST_TS_MS),
            Some(REST_TS_MS),
            MAX_AGE_MS,
        );
        assert!(!alarm.any_stale());
        assert!(alarm.stale_legs().is_empty());
        assert!(alarm.refresh_plan().is_empty());
    }

    #[test]
    fn market_not_resting_legs_never_alarm() {
        // Neither leg resting -> NotResting on both, no alarm even at zero age.
        let alarm = MarketStaleQuoteAlarm::evaluate(REST_TS_MS, None, None, 0);
        assert_eq!(alarm.verdict(Leg::Yes).staleness, LegStaleness::NotResting);
        assert_eq!(alarm.verdict(Leg::No).staleness, LegStaleness::NotResting);
        assert!(!alarm.any_stale());
        assert!(alarm.stale_legs().is_empty());
        assert!(alarm.refresh_plan().is_empty());
    }

    #[test]
    fn stale_legs_and_refresh_plan_are_yes_then_no_ordered() {
        // Both legs stale (one over-age, one out-of-order): the alarm visits YES
        // before NO deterministically in both the verdict list and the plan.
        let alarm = MarketStaleQuoteAlarm::evaluate(
            REST_TS_MS,
            Some(REST_TS_MS - MAX_AGE_MS), // YES over-age -> Stale
            Some(REST_TS_MS + 1),          // NO rest is in the future -> Stale (out-of-order)
            MAX_AGE_MS,
        );
        let stale = alarm.stale_legs();
        assert_eq!(stale.len(), 2);
        assert_eq!(stale[0].leg, Leg::Yes);
        assert_eq!(stale[1].leg, Leg::No);
        assert_eq!(
            alarm.refresh_plan(),
            vec![
                MarketAction::Leg {
                    leg: Leg::Yes,
                    action: LifecycleAction::Cancel,
                },
                MarketAction::Leg {
                    leg: Leg::No,
                    action: LifecycleAction::Cancel,
                },
            ]
        );
    }

    #[test]
    fn only_the_stale_side_appears_in_the_plan() {
        // YES not resting, NO stale: the plan holds exactly the NO cancel.
        let alarm = MarketStaleQuoteAlarm::evaluate(
            REST_TS_MS + MAX_AGE_MS,
            None,
            Some(REST_TS_MS),
            MAX_AGE_MS,
        );
        assert_eq!(
            alarm.refresh_plan(),
            vec![MarketAction::Leg {
                leg: Leg::No,
                action: LifecycleAction::Cancel,
            }]
        );
        // The stale-leg verdict list agrees: just the NO leg.
        let stale = alarm.stale_legs();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].leg, Leg::No);
        assert_eq!(stale[0].staleness, LegStaleness::Stale);
    }
}
