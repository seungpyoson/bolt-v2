//! Pure governor for the binary-oracle maker (W3 — kill predicates + graduated
//! quoting states, FR-023).
//!
//! The maker model (`maker_model` / `maker_quote`) decides *prices*; this module
//! decides *posture* — whether, and how, to quote at all given live market
//! conditions and inventory. It is the W3 maker-quoting governor, distinct from
//! (and complementary to) any equity-drawdown loss/circuit-breaker governor: that
//! one halts on realised loss, this one shapes quoting from market state. They
//! compose at the NT shell (a later slice); neither owns the other.
//!
//! FR-023 requires the posture be a **graduated state**, not a single boolean,
//! and the kill predicates (σ-floor / basis-cap / τ-floor) be **TOML thresholds
//! that fail closed**. So every input is checked for finiteness and a non-finite
//! input trips its predicate (fail closed → hard flat); the thresholds are
//! validated once at construction (no `Default`), so a non-finite or negative
//! threshold can never silently disable a guard. Pure integer/float logic over
//! caller-supplied values — no NautilusTrader type, exhaustively unit-testable.

use crate::bolt_v3_numeric::ZERO_F64;

/// Why the maker hard-flattens. Every reason maps to the same action — cancel
/// all resting quotes and flatten — and the variant is only for ops/logging.
/// Evaluated most-fundamental-first (the precedence is documented on each
/// variant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillReason {
    /// Time-to-expiry below the floor: a binary's variance blows up ~1/√τ into
    /// expiry and settlement is imminent — the most urgent pull.
    TauFloor,
    /// Realised vol below the floor: a frozen/stale feed collapses σ and rails
    /// the N(d2) digital to 0/1, so the fair value is degenerate.
    SigmaFloor,
    /// |oracle fair − venue mid| above the cap: our fair value and the market
    /// have diverged too far to quote against safely (we are wrong, or the venue
    /// dislocated).
    BasisCap,
}

/// Graduated maker posture (FR-023 — explicitly not a single boolean).
///
/// W3 (this module) resolves only the market-and-inventory-driven postures —
/// `Quoting`, `ReduceOnly`, and `HardFlat`. `CancelOnly` (W6 ops triggers:
/// maintenance window, reconnect-in-progress) and `SoftHold` (W7
/// reward-eligibility preservation) are part of the required vocabulary but are
/// produced by their own later slices; safety always wins over reward, so W3
/// never downgrades a hard kill to a reward-preserving hold (FR-060).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerGovernorState {
    /// Normal two-sided quoting.
    Quoting,
    /// Reward-preserving soft hold: keep minimal resting quotes for reward
    /// eligibility but add no new directional risk. Resolved by the W7 reward
    /// layer; W3 never produces it.
    SoftHold,
    /// Quote only the inventory-reducing side — |net| is at or over the
    /// reduce-only cap.
    ReduceOnly,
    /// Cancel resting quotes and submit none. Resolved by W6 ops triggers;
    /// W3 never produces it.
    CancelOnly,
    /// Cancel all resting quotes and flatten inventory — a kill predicate
    /// tripped. Carries the reason for ops.
    HardFlat(KillReason),
}

/// Fail-closed thresholds for the governor, all TOML-sourced by the NT shell.
/// Constructed via [`KillThresholds::new`] (no `Default`: the bolt-v3
/// legacy-default fence forbids it, and a governor must name its resolved
/// config rather than inherit zeros).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KillThresholds {
    sigma_floor: f64,
    basis_cap: f64,
    tau_floor: f64,
    reduce_only_cap: f64,
}

impl KillThresholds {
    /// Build validated thresholds. Returns `None` if any is non-finite or
    /// negative — a floor/cap must be a real, non-negative bound. Validating at
    /// construction means [`MakerGovernor::resolve`] can never be silently
    /// disabled by a `NaN` threshold (e.g. `x < NaN` is always false, which
    /// would fail *open*).
    pub fn new(
        sigma_floor: f64,
        basis_cap: f64,
        tau_floor: f64,
        reduce_only_cap: f64,
    ) -> Option<Self> {
        for v in [sigma_floor, basis_cap, tau_floor, reduce_only_cap] {
            if !v.is_finite() || v < ZERO_F64 {
                return None;
            }
        }
        Some(Self {
            sigma_floor,
            basis_cap,
            tau_floor,
            reduce_only_cap,
        })
    }
}

/// Live market + inventory inputs read each evaluation. Any of the market inputs
/// may be non-finite (a frozen/garbage feed); the governor fails closed on that.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GovernorInputs {
    /// Realised-vol estimate of the underlying.
    pub sigma: f64,
    /// The maker's oracle fair value (probability of the YES/up outcome).
    pub oracle_fair: f64,
    /// The venue's current order-book mid for the YES/up outcome.
    pub venue_mid: f64,
    /// Time-to-expiry in the configured unit (same unit as `tau_floor`).
    pub tau: f64,
    /// Signed net directional inventory (from `MakerInventory::net_position`).
    pub net_position: f64,
}

/// The maker quoting governor: fixed validated thresholds, evaluated against
/// live inputs each tick to yield a graduated posture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerGovernor {
    thresholds: KillThresholds,
}

impl MakerGovernor {
    /// A governor with the given validated thresholds.
    pub fn new(thresholds: KillThresholds) -> Self {
        Self { thresholds }
    }

    /// Resolve the current posture, fail-closed.
    ///
    /// Precedence (most fundamental first): τ-floor → σ-floor → basis-cap →
    /// inventory. Any market input that is non-finite or breaches its threshold
    /// trips a hard flat; otherwise inventory at/over the reduce-only cap (or a
    /// non-finite net, which cannot occur given `MakerInventory`'s validated
    /// fills but is guarded) restricts to inventory-reducing quotes; otherwise
    /// normal two-sided quoting.
    pub fn resolve(&self, inputs: GovernorInputs) -> MakerGovernorState {
        if !inputs.tau.is_finite() || inputs.tau < self.thresholds.tau_floor {
            return MakerGovernorState::HardFlat(KillReason::TauFloor);
        }
        if !inputs.sigma.is_finite() || inputs.sigma < self.thresholds.sigma_floor {
            return MakerGovernorState::HardFlat(KillReason::SigmaFloor);
        }
        let basis = inputs.oracle_fair - inputs.venue_mid;
        if !basis.is_finite() || basis.abs() > self.thresholds.basis_cap {
            return MakerGovernorState::HardFlat(KillReason::BasisCap);
        }
        if !inputs.net_position.is_finite()
            || inputs.net_position.abs() >= self.thresholds.reduce_only_cap
        {
            return MakerGovernorState::ReduceOnly;
        }
        MakerGovernorState::Quoting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A permissive baseline: floors at 0, a wide basis cap, a generous
    /// inventory cap — every guard off, so the base case quotes.
    fn lenient() -> KillThresholds {
        KillThresholds::new(0.0, 1.0, 0.0, 100.0).expect("valid thresholds")
    }

    /// Healthy inputs that pass every guard under `lenient()`.
    fn healthy() -> GovernorInputs {
        GovernorInputs {
            sigma: 0.20,
            oracle_fair: 0.55,
            venue_mid: 0.54,
            tau: 3_600.0,
            net_position: 0.0,
        }
    }

    #[test]
    fn healthy_inputs_quote_two_sided() {
        let gov = MakerGovernor::new(lenient());
        assert_eq!(gov.resolve(healthy()), MakerGovernorState::Quoting);
    }

    #[test]
    fn tau_below_floor_hard_flats() {
        let gov = MakerGovernor::new(KillThresholds::new(0.0, 1.0, 60.0, 100.0).unwrap());
        let mut i = healthy();
        i.tau = 30.0; // below the 60s floor
        assert_eq!(
            gov.resolve(i),
            MakerGovernorState::HardFlat(KillReason::TauFloor)
        );
    }

    #[test]
    fn sigma_below_floor_hard_flats() {
        let gov = MakerGovernor::new(KillThresholds::new(0.05, 1.0, 0.0, 100.0).unwrap());
        let mut i = healthy();
        i.sigma = 0.01; // below the 0.05 floor
        assert_eq!(
            gov.resolve(i),
            MakerGovernorState::HardFlat(KillReason::SigmaFloor)
        );
    }

    #[test]
    fn basis_above_cap_hard_flats() {
        let gov = MakerGovernor::new(KillThresholds::new(0.0, 0.02, 0.0, 100.0).unwrap());
        let mut i = healthy();
        i.oracle_fair = 0.55;
        i.venue_mid = 0.50; // |0.05| > 0.02 cap
        assert_eq!(
            gov.resolve(i),
            MakerGovernorState::HardFlat(KillReason::BasisCap)
        );
    }

    #[test]
    fn non_finite_market_inputs_fail_closed() {
        let gov = MakerGovernor::new(lenient());
        let cases: [(&str, fn(&mut GovernorInputs)); 4] = [
            ("tau", |i| i.tau = f64::NAN),
            ("sigma", |i| i.sigma = f64::INFINITY),
            ("oracle_fair", |i| i.oracle_fair = f64::NAN),
            ("venue_mid", |i| i.venue_mid = f64::NAN),
        ];
        for (label, mutate) in cases {
            let mut i = healthy();
            mutate(&mut i);
            assert!(
                matches!(gov.resolve(i), MakerGovernorState::HardFlat(_)),
                "{label} non-finite must hard-flat"
            );
        }
    }

    #[test]
    fn kill_precedence_is_tau_then_sigma_then_basis() {
        // All three trip at once; τ is reported first, then σ, then basis.
        let gov = MakerGovernor::new(KillThresholds::new(0.05, 0.02, 60.0, 100.0).unwrap());
        let all_bad = GovernorInputs {
            sigma: 0.01,
            oracle_fair: 0.55,
            venue_mid: 0.50,
            tau: 30.0,
            net_position: 0.0,
        };
        assert_eq!(
            gov.resolve(all_bad),
            MakerGovernorState::HardFlat(KillReason::TauFloor)
        );
        // τ healthy: σ wins over basis.
        let mut sigma_and_basis = all_bad;
        sigma_and_basis.tau = 3_600.0;
        assert_eq!(
            gov.resolve(sigma_and_basis),
            MakerGovernorState::HardFlat(KillReason::SigmaFloor)
        );
    }

    #[test]
    fn inventory_at_or_over_cap_is_reduce_only() {
        let gov = MakerGovernor::new(KillThresholds::new(0.0, 1.0, 0.0, 10.0).unwrap());
        let mut at_cap = healthy();
        at_cap.net_position = 10.0; // exactly the cap
        assert_eq!(gov.resolve(at_cap), MakerGovernorState::ReduceOnly);
        let mut over_cap_short = healthy();
        over_cap_short.net_position = -11.0;
        assert_eq!(gov.resolve(over_cap_short), MakerGovernorState::ReduceOnly);
        let mut under_cap = healthy();
        under_cap.net_position = 9.0;
        assert_eq!(gov.resolve(under_cap), MakerGovernorState::Quoting);
    }

    #[test]
    fn non_finite_net_fails_closed_to_reduce_only() {
        let gov = MakerGovernor::new(lenient());
        let mut i = healthy();
        i.net_position = f64::NAN;
        assert_eq!(gov.resolve(i), MakerGovernorState::ReduceOnly);
    }

    #[test]
    fn a_hard_kill_outranks_an_inventory_breach() {
        // Inventory is over the cap AND σ is below the floor: the hard kill wins.
        let gov = MakerGovernor::new(KillThresholds::new(0.05, 1.0, 0.0, 10.0).unwrap());
        let mut i = healthy();
        i.sigma = 0.01;
        i.net_position = 50.0;
        assert_eq!(
            gov.resolve(i),
            MakerGovernorState::HardFlat(KillReason::SigmaFloor)
        );
    }

    #[test]
    fn thresholds_reject_non_finite_or_negative() {
        assert!(KillThresholds::new(f64::NAN, 1.0, 0.0, 10.0).is_none());
        assert!(KillThresholds::new(0.0, f64::INFINITY, 0.0, 10.0).is_none());
        assert!(KillThresholds::new(0.0, 1.0, -1.0, 10.0).is_none());
        assert!(KillThresholds::new(0.0, 1.0, 0.0, -10.0).is_none());
        assert!(KillThresholds::new(0.0, 0.0, 0.0, 0.0).is_some());
    }

    #[test]
    fn graduated_vocabulary_has_all_four_non_quoting_postures() {
        // FR-023: the posture is graduated, not a boolean. The full vocabulary
        // (incl. CancelOnly/SoftHold, resolved by W6/W7) is distinct.
        let states = [
            MakerGovernorState::Quoting,
            MakerGovernorState::SoftHold,
            MakerGovernorState::ReduceOnly,
            MakerGovernorState::CancelOnly,
            MakerGovernorState::HardFlat(KillReason::TauFloor),
        ];
        for (a_idx, a) in states.iter().enumerate() {
            for (b_idx, b) in states.iter().enumerate() {
                assert_eq!(a_idx == b_idx, a == b, "states must be mutually distinct");
            }
        }
    }
}
