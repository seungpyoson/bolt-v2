//! Pure reward-eligibility pull policy for the binary-oracle maker
//! (W7 — FR-060: the `SoftHold` producer; reward continuity vs safety).
//!
//! Reward programs pay for *continuous resting* presence, so when a market is a
//! funded reward candidate the maker prefers to keep minimal resting quotes for
//! reward continuity rather than pull them. This module turns that preference
//! into the governor vocabulary's reward-preserving posture
//! ([`MakerGovernorState::SoftHold`]) — the W3 governor declares it but never
//! produces it; this is its producer, the exact analogue of W6's maintenance
//! gate producing `CancelOnly`.
//!
//! ## Safety always wins — structurally, not by policy
//!
//! This module CANNOT override safety. It only ever emits `Quoting` or
//! `SoftHold`; it never produces a kill and never a posture more permissive than
//! `Quoting`. The "safety wins" guarantee lives in the shell, which folds
//! [`reward_governor_state`] through the existing `most_restrictive` precedence
//! alongside the W3 market governor and the W6 maintenance gate. Because
//! `SoftHold` ranks above `Quoting` but below `ReduceOnly`/`CancelOnly`/
//! `HardFlat`, the fold can only ever ESCALATE a reward `SoftHold` up to a real
//! kill — never soften one. A reward signal therefore can never keep quotes
//! resting against a safety kill; reward loss is accepted, not traded against
//! safety. This mirrors how W6 maintenance composes its `CancelOnly`.
//!
//! Pure: no NautilusTrader type, no async, no I/O. No `Default`. Fail-closed: a
//! non-finite reward rate yields `None`, which the mapping sends to `Quoting`
//! (the no-veto state) so a bad feed never fabricates a hold.

use crate::bolt_v3_numeric::is_positive_finite;
use crate::strategies::maker_governor::MakerGovernorState;

/// The reward-eligibility posture for one market.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewardPosture {
    /// No funded reward pool, or the market is not a phantom-LP candidate —
    /// reward gives no reason to hold quotes.
    Ineligible,
    /// A funded, eligible reward market — prefer to keep minimal resting quotes
    /// for reward continuity (a soft hold, never a kill).
    Eligible,
}

/// The reward-eligibility inputs for one market: the pool's native daily payout
/// rate and whether the market is a phantom-LP candidate (from
/// [`crate::strategies::maker_reward_phantom_lp::is_phantom_lp_candidate`]).
///
/// Plain inputs view (no `Default`): the caller projects the live reward feed +
/// the candidate predicate here at the call site, so this module never sees the
/// feed, NT, or async.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RewardEligibilityInputs {
    /// The reward pool's native daily payout rate for this market.
    pub native_daily_rate: f64,
    /// Whether this market is a phantom-LP candidate this sample.
    pub candidate: bool,
}

/// Resolve the reward posture, fail-closed.
///
/// Returns `Some(Eligible)` only when the market is a candidate AND the native
/// daily rate is positive and finite (a live, funded pool); `Some(Ineligible)`
/// when not a candidate or the rate is non-positive; and `None` when the rate is
/// non-finite — the caller maps `None` to the no-veto state, so a bad feed never
/// fabricates a hold.
pub fn reward_posture(inputs: RewardEligibilityInputs) -> Option<RewardPosture> {
    if !inputs.native_daily_rate.is_finite() {
        return None;
    }
    if inputs.candidate && is_positive_finite(inputs.native_daily_rate) {
        Some(RewardPosture::Eligible)
    } else {
        Some(RewardPosture::Ineligible)
    }
}

/// Map the reward posture onto the shared governor vocabulary.
///
/// `Some(Eligible)` → [`MakerGovernorState::SoftHold`] (reward-preserving);
/// `Some(Ineligible)` and `None` → [`MakerGovernorState::Quoting`] (no veto).
/// Reward can ONLY emit `Quoting` or `SoftHold` — never a kill, never more
/// permissive than `Quoting`. Safety dominance is enforced by the shell folding
/// this through `most_restrictive` (where `SoftHold` ranks below every kill),
/// exactly as the W6 maintenance gate composes its `CancelOnly`.
pub fn reward_governor_state(posture: Option<RewardPosture>) -> MakerGovernorState {
    match posture {
        Some(RewardPosture::Eligible) => MakerGovernorState::SoftHold,
        Some(RewardPosture::Ineligible) | None => MakerGovernorState::Quoting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn funded_candidate_soft_holds() {
        let inputs = RewardEligibilityInputs {
            native_daily_rate: 12.5,
            candidate: true,
        };
        assert_eq!(reward_posture(inputs), Some(RewardPosture::Eligible));
        assert_eq!(
            reward_governor_state(reward_posture(inputs)),
            MakerGovernorState::SoftHold
        );
    }

    #[test]
    fn non_candidate_or_unfunded_is_ineligible_and_quotes() {
        let not_candidate = RewardEligibilityInputs {
            native_daily_rate: 12.5,
            candidate: false,
        };
        let unfunded = RewardEligibilityInputs {
            native_daily_rate: 0.0,
            candidate: true,
        };
        assert_eq!(
            reward_posture(not_candidate),
            Some(RewardPosture::Ineligible)
        );
        assert_eq!(reward_posture(unfunded), Some(RewardPosture::Ineligible));
        assert_eq!(
            reward_governor_state(reward_posture(not_candidate)),
            MakerGovernorState::Quoting
        );
        assert_eq!(
            reward_governor_state(reward_posture(unfunded)),
            MakerGovernorState::Quoting
        );
    }

    #[test]
    fn non_finite_rate_fails_closed_to_quoting() {
        let bad = RewardEligibilityInputs {
            native_daily_rate: f64::NAN,
            candidate: true,
        };
        assert_eq!(reward_posture(bad), None);
        // None maps to Quoting (no spurious hold); the base/safety governor decides.
        assert_eq!(reward_governor_state(None), MakerGovernorState::Quoting);
    }

    #[test]
    fn reward_never_emits_a_kill_or_more_permissive_than_quoting() {
        // Exhaustive over the posture domain: the only outputs are Quoting/SoftHold.
        for posture in [
            Some(RewardPosture::Eligible),
            Some(RewardPosture::Ineligible),
            None,
        ] {
            let state = reward_governor_state(posture);
            assert!(matches!(
                state,
                MakerGovernorState::Quoting | MakerGovernorState::SoftHold
            ));
        }
    }
}
