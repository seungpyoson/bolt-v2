//! Typed maker configuration schema — the single TOML-sourced home for every
//! knob the pure binary-oracle maker modules consume.
//!
//! The maker's pricing, governance, inventory, and throttle logic is split
//! across the pure `maker_*` modules ([`crate::strategies::maker_governor`],
//! [`crate::strategies::maker_quote`], [`crate::strategies::maker_offsets`],
//! [`crate::strategies::maker_microprice`], [`crate::strategies::maker_model`],
//! [`crate::strategies::maker_inventory`], and
//! [`crate::strategies::requote_budget`]). Each of those modules is a no-clock,
//! no-I/O, fail-closed primitive that takes its bounds as plain inputs. This
//! module is the *one* place those inputs are deserialized from operator TOML
//! and validated before they reach any pricing call — the configuration
//! counterpart of the taker's
//! [`crate::bolt_v3_archetypes::binary_oracle_edge_taker::ParametersBlock`].
//!
//! Design contract (matching the taker's archetype `[parameters]` block, NO
//! DUAL PATHS):
//!
//! - [`MakerParametersBlock`] is a `serde::Deserialize` struct with
//!   `#[serde(deny_unknown_fields)]`, so a misspelled or stale knob is a loud
//!   parse error rather than a silently-ignored field.
//! - [`MakerParametersBlock::validate`] is **fail-loud and error-collecting**:
//!   it accumulates *every* out-of-domain knob into a `Vec<String>` (it does
//!   not stop at the first), so an operator sees all of a bad config's problems
//!   in one pass — the same collect-all contract as the taker's
//!   `validate_parameter_bounds`.
//! - Validation is performed by **constructing the pure types where a guarded
//!   constructor exists** rather than re-implementing their bounds. The
//!   governor thresholds are validated by calling
//!   [`crate::strategies::maker_governor::KillThresholds::new`] and failing if
//!   it returns `None` — that constructor is the single source of truth for the
//!   `sigma_floor` / `basis_cap` / `tau_floor` / `reduce_only_cap` domains, so
//!   their bounds are never duplicated here. The remaining knobs are checked
//!   against the documented domains of the free functions that consume them,
//!   reusing the shared predicates and constants from
//!   [`crate::bolt_v3_numeric`] so the checks cannot drift from the consumers'
//!   guards.
//! - On success [`MakerParametersBlock::validate`] returns a
//!   [`ValidatedMakerConfig`], which carries the already-constructed
//!   [`KillThresholds`](crate::strategies::maker_governor::KillThresholds) (so
//!   the governor is built from the single validated value, not re-validated
//!   downstream) alongside the validated scalar knobs.
//!
//! This module is pure: no NautilusTrader type, no async, no I/O, no clock, and
//! no `Default` impl (the bolt-v3 legacy-default fence forbids it — the only
//! constructors are the named [`MakerParametersBlock::validate`] →
//! [`ValidatedMakerConfig`] path). Registering a strategy builder and wiring
//! these values into the NT runtime shell is the integrator's job and is out of
//! scope here.

use serde::Deserialize;

use crate::bolt_v3_numeric::{HALF_F64, UNIT_F64, ZERO_F64, is_positive_finite};
use crate::strategies::maker_governor::KillThresholds;

/// Raw, TOML-sourced maker parameter block.
///
/// Deserialized from the maker strategy's `[parameters]` table with
/// `deny_unknown_fields`, then validated via [`Self::validate`]. Holds every
/// knob the pure maker modules consume, grouped by the consuming layer (see the
/// per-field docs). No `Default`: a maker must name its full parameter set;
/// inheriting zeros would silently disable the fail-closed guards.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MakerParametersBlock {
    /// Half-collar for the open-interval quote-leg guard
    /// ([`crate::bolt_v3_numeric::sanitize_open_probability`],
    /// [`crate::strategies::maker_offsets::compose_binary_leg_prices`]). An
    /// emitted leg must land strictly inside `(eps, 1 − eps)`. Required domain:
    /// the open interval `0 < eps < 0.5` (an `eps` at or above `0.5` collapses
    /// the admissible interval).
    pub eps: f64,

    /// Reference time-to-expiry horizon for the time-widening factor
    /// ([`crate::strategies::maker_offsets::time_widening_factor`]). The spread
    /// widens like `sqrt(reference_tau / tau)` as expiry approaches. Required
    /// domain: strictly positive and finite (a non-positive horizon is
    /// meaningless and makes the ratio degenerate).
    pub reference_tau: f64,

    /// Upper clamp on the time-widening factor
    /// ([`crate::strategies::maker_offsets::time_widening_factor`]). The factor
    /// is clamped to `[1.0, time_widen_cap]`. Required domain: finite and
    /// `>= 1.0` (a cap below `1.0` would force the maker to *tighten* into
    /// expiry, the opposite of the intended widening).
    pub time_widen_cap: f64,

    /// Weight of the order-book micro-price when blending it toward the oracle
    /// fair ([`crate::strategies::maker_microprice::micro_price_anchor`]).
    /// Required domain: finite and within the closed interval `[0.0, 1.0]`
    /// (`0.0` ignores the book, `1.0` follows it fully).
    pub micro_weight: f64,

    /// Governor realised-vol floor — below it the governor hard-flats
    /// ([`KillThresholds`](crate::strategies::maker_governor::KillThresholds)).
    /// Domain owned by [`KillThresholds::new`] (finite, non-negative); validated
    /// by constructing the thresholds, never re-bounded here.
    pub sigma_floor: f64,

    /// Governor basis cap — `|oracle_fair − venue_mid|` above it hard-flats
    /// ([`KillThresholds`](crate::strategies::maker_governor::KillThresholds)).
    /// Domain owned by [`KillThresholds::new`].
    pub basis_cap: f64,

    /// Governor time-to-expiry floor — `tau` below it hard-flats
    /// ([`KillThresholds`](crate::strategies::maker_governor::KillThresholds)).
    /// Domain owned by [`KillThresholds::new`].
    pub tau_floor: f64,

    /// Governor reduce-only inventory cap — `|net_position|` at or above it
    /// restricts to inventory-reducing quotes
    /// ([`KillThresholds`](crate::strategies::maker_governor::KillThresholds)).
    /// Domain owned by [`KillThresholds::new`].
    pub reduce_only_cap: f64,

    /// Inventory-skew gain — price units of skew per unit of net position
    /// ([`crate::strategies::maker_model::inventory_skew`]). Required domain:
    /// finite and `>= 0.0` (a gain of zero disables the skew; a negative gain
    /// would lean *into* inventory, amplifying directional risk).
    pub skew_gain: f64,

    /// Hard cap on `|net_position|` for the inventory-skew model
    /// ([`crate::strategies::maker_model::inventory_skew`]). Required domain:
    /// strictly positive and finite (a non-positive cap is not a real position
    /// limit).
    pub position_cap: f64,

    /// Lower bound on the resolved quote half-spread
    /// ([`crate::strategies::maker_quote::resolve_band`]). Required domain:
    /// finite and `>= 0.0`.
    pub half_spread_floor: f64,

    /// Upper bound on the resolved quote half-spread
    /// ([`crate::strategies::maker_quote::resolve_band`]). Required domain:
    /// finite and `>= half_spread_floor` (an upper bound below the floor is an
    /// empty band).
    pub max_half_spread: f64,

    /// Minimum gap between two requote commands, in milliseconds
    /// ([`crate::strategies::requote_budget::RequoteBudget`]). A `u64`, so it is
    /// non-negative by type; the throttle accepts any value (`0` simply imposes
    /// no inter-requote spacing), so there is no further domain constraint.
    pub requote_min_interval_ms: u64,

    /// Fraction of incoming flow assumed informed, for the Glosten–Milgrom
    /// adverse-selection half-spread
    /// ([`crate::strategies::maker_model::gm_half_spread`]). Required domain:
    /// finite and within the closed interval `[0.0, 1.0]`.
    pub informed_fraction: f64,
}

/// A validated maker configuration.
///
/// Returned by [`MakerParametersBlock::validate`] on success. It carries the
/// already-constructed [`KillThresholds`] — the single validated governor-bound
/// value, so the runtime builds the governor from this rather than re-running
/// the threshold checks — alongside the validated scalar knobs each pure module
/// consumes. Constructed only through validation (no `Default`, no public field
/// construction path), so a `ValidatedMakerConfig` is proof the knobs passed
/// every domain check.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedMakerConfig {
    /// Governor thresholds, already constructed via [`KillThresholds::new`].
    kill_thresholds: KillThresholds,
    eps: f64,
    reference_tau: f64,
    time_widen_cap: f64,
    micro_weight: f64,
    skew_gain: f64,
    position_cap: f64,
    half_spread_floor: f64,
    max_half_spread: f64,
    requote_min_interval_ms: u64,
    informed_fraction: f64,
}

impl ValidatedMakerConfig {
    /// The validated governor thresholds (the single source for building the
    /// [`crate::strategies::maker_governor::MakerGovernor`]).
    pub fn kill_thresholds(&self) -> KillThresholds {
        self.kill_thresholds
    }

    /// Open-interval quote-leg half-collar (`0 < eps < 0.5`).
    pub fn eps(&self) -> f64 {
        self.eps
    }

    /// Reference time-to-expiry horizon for time-widening.
    pub fn reference_tau(&self) -> f64 {
        self.reference_tau
    }

    /// Upper clamp on the time-widening factor (`>= 1.0`).
    pub fn time_widen_cap(&self) -> f64 {
        self.time_widen_cap
    }

    /// Micro-price blend weight (`[0, 1]`).
    pub fn micro_weight(&self) -> f64 {
        self.micro_weight
    }

    /// Inventory-skew gain (`>= 0`).
    pub fn skew_gain(&self) -> f64 {
        self.skew_gain
    }

    /// Hard cap on `|net_position|` for the inventory-skew model (`> 0`).
    pub fn position_cap(&self) -> f64 {
        self.position_cap
    }

    /// Lower bound on the resolved quote half-spread (`>= 0`).
    pub fn half_spread_floor(&self) -> f64 {
        self.half_spread_floor
    }

    /// Upper bound on the resolved quote half-spread (`>= half_spread_floor`).
    pub fn max_half_spread(&self) -> f64 {
        self.max_half_spread
    }

    /// Minimum gap between requote commands, in milliseconds.
    pub fn requote_min_interval_ms(&self) -> u64 {
        self.requote_min_interval_ms
    }

    /// Informed-flow fraction for the GM half-spread (`[0, 1]`).
    pub fn informed_fraction(&self) -> f64 {
        self.informed_fraction
    }
}

impl MakerParametersBlock {
    /// Validate every knob, fail-loud and collecting **all** errors.
    ///
    /// Returns `Ok(ValidatedMakerConfig)` only when every field is in its
    /// documented domain; otherwise `Err(messages)` where `messages` holds one
    /// human-readable line per failing knob (so an operator sees every problem
    /// at once, not just the first). Each message is prefixed with `context`
    /// (e.g. the strategy instance id) in the `"{context}: parameters.<field>
    /// ..."` shape used by the taker archetype validator.
    ///
    /// The governor thresholds are validated by **constructing**
    /// [`KillThresholds`] via [`KillThresholds::new`] — the single source of
    /// truth for their bounds — and the result is carried into the returned
    /// [`ValidatedMakerConfig`]. The remaining knobs are checked against the
    /// documented domains of the free functions that consume them, reusing the
    /// shared predicates/constants from [`crate::bolt_v3_numeric`].
    pub fn validate(&self, context: &str) -> Result<ValidatedMakerConfig, Vec<String>> {
        let mut errors: Vec<String> = Vec::new();

        // `eps` — open-interval half-collar `(0, 0.5)`. Same domain the
        // `sanitize_open_probability` quote-leg guard enforces on `eps`.
        if !(self.eps.is_finite() && self.eps > ZERO_F64 && self.eps < HALF_F64) {
            errors.push(format!(
                "{context}: parameters.eps must be a finite value in the open interval (0, 0.5): `{}`",
                self.eps
            ));
        }

        // `reference_tau` — strictly positive finite (the `time_widening_factor`
        // horizon).
        if !is_positive_finite(self.reference_tau) {
            errors.push(format!(
                "{context}: parameters.reference_tau must be a positive finite horizon: `{}`",
                self.reference_tau
            ));
        }

        // `time_widen_cap` — finite and `>= 1.0` (the `time_widening_factor`
        // upper clamp; a cap below 1.0 would tighten into expiry).
        if !self.time_widen_cap.is_finite() || self.time_widen_cap < UNIT_F64 {
            errors.push(format!(
                "{context}: parameters.time_widen_cap must be a finite value >= 1.0: `{}`",
                self.time_widen_cap
            ));
        }

        // `micro_weight` — finite and within `[0, 1]` (the `micro_price_anchor`
        // blend weight).
        if !self.micro_weight.is_finite() || !(ZERO_F64..=UNIT_F64).contains(&self.micro_weight) {
            errors.push(format!(
                "{context}: parameters.micro_weight must be a finite value in [0.0, 1.0]: `{}`",
                self.micro_weight
            ));
        }

        // Governor thresholds — single source of truth is `KillThresholds::new`.
        // It validates the four as one group and returns a bare Option (no
        // per-threshold reason), so a rejection surfaces as ONE combined error
        // listing all four values; validate() cannot attribute the failure to a
        // specific threshold. That coarse granularity is the deliberate cost of
        // not duplicating the constructor's bounds here.
        let kill_thresholds = KillThresholds::new(
            self.sigma_floor,
            self.basis_cap,
            self.tau_floor,
            self.reduce_only_cap,
        );
        if kill_thresholds.is_none() {
            errors.push(format!(
                "{context}: parameters governor thresholds rejected by KillThresholds::new — each of sigma_floor (`{}`), basis_cap (`{}`), tau_floor (`{}`), reduce_only_cap (`{}`) must be a finite, non-negative bound",
                self.sigma_floor, self.basis_cap, self.tau_floor, self.reduce_only_cap
            ));
        }

        // `skew_gain` — finite and `>= 0` (the `inventory_skew` gain; negative
        // would lean into inventory).
        if !self.skew_gain.is_finite() || self.skew_gain < ZERO_F64 {
            errors.push(format!(
                "{context}: parameters.skew_gain must be a finite value >= 0.0: `{}`",
                self.skew_gain
            ));
        }

        // `position_cap` — strictly positive finite (the `inventory_skew` hard
        // cap).
        if !is_positive_finite(self.position_cap) {
            errors.push(format!(
                "{context}: parameters.position_cap must be a positive finite share count: `{}`",
                self.position_cap
            ));
        }

        // Half-spread band — `resolve_band` requires a finite, non-negative
        // floor and a finite `max_half_spread >= half_spread_floor`. Report each
        // breach independently.
        let floor_ok = self.half_spread_floor.is_finite() && self.half_spread_floor >= ZERO_F64;
        if !floor_ok {
            errors.push(format!(
                "{context}: parameters.half_spread_floor must be a finite value >= 0.0: `{}`",
                self.half_spread_floor
            ));
        }
        if !self.max_half_spread.is_finite() {
            errors.push(format!(
                "{context}: parameters.max_half_spread must be a finite value: `{}`",
                self.max_half_spread
            ));
        } else if floor_ok && self.max_half_spread < self.half_spread_floor {
            errors.push(format!(
                "{context}: parameters.max_half_spread (`{}`) must be >= parameters.half_spread_floor (`{}`)",
                self.max_half_spread, self.half_spread_floor
            ));
        }

        // `informed_fraction` — finite and within `[0, 1]` (the `gm_half_spread`
        // / `gm_binary_quote` informed-flow fraction).
        if !self.informed_fraction.is_finite()
            || !(ZERO_F64..=UNIT_F64).contains(&self.informed_fraction)
        {
            errors.push(format!(
                "{context}: parameters.informed_fraction must be a finite value in [0.0, 1.0]: `{}`",
                self.informed_fraction
            ));
        }

        // `requote_min_interval_ms` is a `u64`: non-negative by type and the
        // throttle accepts any value, so it carries no further domain check.

        match kill_thresholds {
            Some(kill_thresholds) if errors.is_empty() => Ok(ValidatedMakerConfig {
                kill_thresholds,
                eps: self.eps,
                reference_tau: self.reference_tau,
                time_widen_cap: self.time_widen_cap,
                micro_weight: self.micro_weight,
                skew_gain: self.skew_gain,
                position_cap: self.position_cap,
                half_spread_floor: self.half_spread_floor,
                max_half_spread: self.max_half_spread,
                requote_min_interval_ms: self.requote_min_interval_ms,
                informed_fraction: self.informed_fraction,
            }),
            _ => Err(errors),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTEXT: &str = "strategies/maker.toml";

    /// A full, valid maker `[parameters]` block. Literals here are test-only;
    /// the source-fence strips `#[cfg(test)]`, so bare numerics are allowed.
    fn valid_toml() -> &'static str {
        "
eps = 0.01
reference_tau = 3600.0
time_widen_cap = 4.0
micro_weight = 0.3
sigma_floor = 0.05
basis_cap = 0.1
tau_floor = 60.0
reduce_only_cap = 100.0
skew_gain = 0.001
position_cap = 500.0
half_spread_floor = 0.005
max_half_spread = 0.2
requote_min_interval_ms = 500
informed_fraction = 0.25
"
    }

    fn valid_block() -> MakerParametersBlock {
        toml::from_str(valid_toml()).expect("the canonical block must deserialize")
    }

    #[test]
    fn a_full_valid_config_deserializes_and_validates() {
        let block = valid_block();
        let validated = block
            .validate(CONTEXT)
            .expect("a fully valid block must validate");

        // The validated knobs round-trip the raw values.
        assert_eq!(validated.eps(), 0.01);
        assert_eq!(validated.reference_tau(), 3600.0);
        assert_eq!(validated.time_widen_cap(), 4.0);
        assert_eq!(validated.micro_weight(), 0.3);
        assert_eq!(validated.skew_gain(), 0.001);
        assert_eq!(validated.position_cap(), 500.0);
        assert_eq!(validated.half_spread_floor(), 0.005);
        assert_eq!(validated.max_half_spread(), 0.2);
        assert_eq!(validated.requote_min_interval_ms(), 500);
        assert_eq!(validated.informed_fraction(), 0.25);

        // The carried thresholds are exactly what KillThresholds::new produced
        // from the same inputs — single source of truth, not a re-derivation.
        let expected = KillThresholds::new(0.05, 0.1, 60.0, 100.0).unwrap();
        assert_eq!(validated.kill_thresholds(), expected);
    }

    #[test]
    fn deny_unknown_fields_rejects_an_unknown_key() {
        let mut toml = valid_toml().to_string();
        toml.push_str("not_a_real_knob = 1.0\n");
        let result: Result<MakerParametersBlock, _> = toml::from_str(&toml);
        assert!(
            result.is_err(),
            "deny_unknown_fields must reject an unrecognized knob"
        );
    }

    #[test]
    fn a_missing_field_is_a_deserialization_error() {
        // Dropping a required knob must be a loud parse error, not a defaulted
        // zero (there is no Default impl).
        let toml = valid_toml().replace("informed_fraction = 0.25\n", "");
        let result: Result<MakerParametersBlock, _> = toml::from_str(&toml);
        assert!(
            result.is_err(),
            "a missing required knob must fail to parse"
        );
    }

    /// Build a block from the valid base with one field overridden to a raw TOML
    /// value, for the per-knob out-of-range cases.
    fn block_with(field: &str, raw_value: &str) -> MakerParametersBlock {
        let needle_prefix = format!("{field} = ");
        let mut out = String::new();
        let mut replaced = false;
        for line in valid_toml().lines() {
            if line.trim_start().starts_with(&needle_prefix) {
                out.push_str(&format!("{field} = {raw_value}\n"));
                replaced = true;
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        assert!(replaced, "field `{field}` must exist in the base block");
        toml::from_str(&out).expect("override block must still deserialize")
    }

    fn assert_field_error(field: &str, raw_value: &str) {
        let block = block_with(field, raw_value);
        let errors = block
            .validate(CONTEXT)
            .expect_err("an out-of-range knob must fail validation");
        assert!(
            errors.iter().any(|e| e.contains(field)),
            "expected an error mentioning `{field}`, got: {errors:?}"
        );
        assert!(
            errors.iter().all(|e| e.starts_with(CONTEXT)),
            "every error must carry the context prefix, got: {errors:?}"
        );
    }

    #[test]
    fn eps_out_of_open_interval_fails() {
        // At/above 0.5 the open-interval collar is empty; 0.0 is the closed edge.
        assert_field_error("eps", "0.5");
        assert_field_error("eps", "0.0");
        assert_field_error("eps", "-0.01");
    }

    #[test]
    fn non_positive_reference_tau_fails() {
        assert_field_error("reference_tau", "0.0");
        assert_field_error("reference_tau", "-1.0");
    }

    #[test]
    fn time_widen_cap_below_one_fails() {
        assert_field_error("time_widen_cap", "0.9");
    }

    #[test]
    fn micro_weight_outside_unit_interval_fails() {
        assert_field_error("micro_weight", "1.5");
        assert_field_error("micro_weight", "-0.1");
    }

    #[test]
    fn negative_skew_gain_fails() {
        assert_field_error("skew_gain", "-0.001");
    }

    #[test]
    fn non_positive_position_cap_fails() {
        assert_field_error("position_cap", "0.0");
        assert_field_error("position_cap", "-10.0");
    }

    #[test]
    fn negative_half_spread_floor_fails() {
        assert_field_error("half_spread_floor", "-0.001");
    }

    #[test]
    fn max_half_spread_below_floor_fails() {
        // floor 0.005 (from the base) > 0.001 here -> empty band.
        let errors = block_with("max_half_spread", "0.001")
            .validate(CONTEXT)
            .expect_err("an upper bound below the floor must fail");
        assert!(
            errors.iter().any(|e| e.contains("max_half_spread")),
            "expected a max_half_spread error, got: {errors:?}"
        );
    }

    #[test]
    fn informed_fraction_outside_unit_interval_fails() {
        assert_field_error("informed_fraction", "1.1");
        assert_field_error("informed_fraction", "-0.1");
    }

    #[test]
    fn the_reused_kill_thresholds_guard_rejects_bad_thresholds() {
        // A negative governor threshold is rejected by KillThresholds::new — the
        // single source of truth — and surfaced as one collected error. This is
        // exactly the bound the constructor owns; it is not duplicated here.
        let block = block_with("sigma_floor", "-0.05");
        let errors = block
            .validate(CONTEXT)
            .expect_err("a negative governor threshold must fail");
        assert!(
            errors.iter().any(|e| e.contains("KillThresholds::new")),
            "the failure must come from the reused KillThresholds::new guard, got: {errors:?}"
        );
        // And it must agree with the standalone constructor on the same inputs.
        assert!(
            KillThresholds::new(-0.05, 0.1, 60.0, 100.0).is_none(),
            "control: the constructor itself rejects the same inputs"
        );
    }

    #[test]
    fn non_finite_governor_threshold_fails_via_constructor() {
        // NaN must not fail open (x < NaN is always false). KillThresholds::new
        // rejects it; validate() surfaces that.
        let block = block_with("basis_cap", "nan");
        let errors = block
            .validate(CONTEXT)
            .expect_err("a non-finite governor threshold must fail");
        assert!(
            errors.iter().any(|e| e.contains("KillThresholds::new")),
            "a NaN threshold must be rejected by the reused guard, got: {errors:?}"
        );
    }

    #[test]
    fn multiple_bad_fields_collect_multiple_errors() {
        // Three independent breaches in distinct knobs must all appear — the
        // validator collects, it does not stop at the first.
        let mut block = valid_block();
        block.eps = HALF_F64; // open-interval breach
        block.micro_weight = 2.0; // outside [0, 1]
        block.position_cap = ZERO_F64; // non-positive cap

        let errors = block
            .validate(CONTEXT)
            .expect_err("a config with several bad knobs must fail");
        assert!(
            errors.len() >= 3,
            "expected at least three collected errors, got: {errors:?}"
        );
        assert!(errors.iter().any(|e| e.contains("eps")));
        assert!(errors.iter().any(|e| e.contains("micro_weight")));
        assert!(errors.iter().any(|e| e.contains("position_cap")));
    }

    #[test]
    fn governor_threshold_error_and_scalar_errors_collect_together() {
        // A bad governor threshold and a bad scalar knob must both be reported in
        // one pass (the KillThresholds::None branch must not short-circuit the
        // scalar checks).
        let mut block = valid_block();
        block.tau_floor = -60.0; // governor-guard breach
        block.informed_fraction = 5.0; // scalar breach

        let errors = block
            .validate(CONTEXT)
            .expect_err("both classes of error must surface");
        assert!(errors.iter().any(|e| e.contains("KillThresholds::new")));
        assert!(errors.iter().any(|e| e.contains("informed_fraction")));
    }
}
