//! Maker per-market binding subsystem (PR-A, #817, umbrella #488).
//!
//! The maker is multi-market: the operator declares an array of markets in
//! `[[strategies.<id>.parameters.markets]]`, each one mirroring the taker's
//! `MarketSelectionTarget` fields. This module owns the *resolution* step that
//! turns each declared market into a concrete leg binding and feeds the resolved
//! set into the existing generic portfolio planner. It does NOT subscribe, place
//! orders, or run a quote loop — that is PR-B.
//!
//! Two reuse seams, both load-bearing for "NT-first / DRY":
//!
//! 1. Discovery is the shared engine
//!    [`bolt_v3_market_families::select_binary_option_market_from_target`]: for
//!    each declared market this module builds a
//!    [`MarketSelectionTarget`] (exactly as the taker's `selection.rs` does) and
//!    asks the engine for the current `SelectedBinaryOptionMarket`. The engine's
//!    `up_instrument_id` becomes the YES leg and `down_instrument_id` the NO leg
//!    of a [`MakerLegBinding`] (order identities are unset here; PR-B assigns
//!    them).
//! 2. Portfolio policy is the shared planner
//!    [`plan_maker_market_portfolio`]: the resolved markets become
//!    [`MakerMarketCandidate`]s fed into the existing planner to produce a
//!    [`MakerMarketSlotPlan`]. No discovery or portfolio logic is reimplemented.
//!
//! Fail-closed throughout: a market whose family/discovery yields no current
//! market is reported as a per-market resolution miss (not silently dropped),
//! and the declared-set bounds (non-empty, within the concurrency cap, registered
//! family) are enforced in the archetype's go-live gate before this code runs.

use nautilus_model::instruments::InstrumentAny;

use crate::bolt_v3_maker_market_selection::{
    MakerMarketCandidate, MakerMarketPortfolioDecision, MakerMarketPortfolioPolicy,
    MakerMarketSlotState, plan_maker_market_portfolio,
};
use crate::bolt_v3_maker_order_plan::MakerLegBinding;
use crate::bolt_v3_market_families::{
    self, MarketSelectionOutcome, MarketSelectionTarget, SelectedMarketSourceIdentity,
};

/// One operator-declared market the maker should quote, normalized into the
/// shape the shared discovery engine consumes. Mirrors the taker's
/// `MarketSelectionTarget`-building config fields exactly (`family_key`,
/// `underlying_asset`, `cadence_seconds`, `cadence_slug_token`, and the optional
/// static-market overrides), plus a stable `market_key` the portfolio planner
/// keys slots and rotation by.
///
/// `cadence_seconds` is `u64` here (operator-facing, validated `> 0` upstream)
/// and converted to the engine's signed `i64` at resolution, exactly as the
/// taker's `select_configured_market_from_instruments` does; an overflowing
/// conversion fails the market's resolution rather than silently wrapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakerMarketDeclaration {
    pub market_key: String,
    pub family_key: String,
    pub underlying_asset: String,
    pub cadence_seconds: u64,
    pub cadence_slug_token: String,
    pub static_condition_id: Option<String>,
    pub static_yes_outcome: Option<String>,
    pub static_no_outcome: Option<String>,
}

/// A declared market that resolved to a concrete current market via the shared
/// engine. Carries the YES (up) and NO (down) leg bindings the runtime will
/// quote against, plus the per-market interval/family/source identity fields PR-B
/// needs to drive lifecycle and settlement. Leg order identities are unset here;
/// PR-B assigns `active_order`/`next_order` as orders are placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakerResolvedMarketBinding {
    pub market_key: String,
    pub family_key: String,
    pub market_id: String,
    pub yes: MakerLegBinding,
    pub no: MakerLegBinding,
    pub selection_outcome: MarketSelectionOutcome,
    pub source_identity: SelectedMarketSourceIdentity,
    pub start_timestamp_milliseconds: u64,
    pub expiration_timestamp_milliseconds: u64,
    pub seconds_to_end: u64,
}

/// Why a declared market did not produce a binding this resolution pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MakerMarketResolutionMiss {
    /// The declared `cadence_seconds` cannot be represented as the engine's
    /// signed `i64` cadence, so the market cannot be queried. Fails closed
    /// rather than silently wrapping to a negative cadence.
    CadenceSecondsOutOfRange {
        market_key: String,
        cadence_seconds: u64,
    },
    /// The shared discovery engine found no current market for this declaration
    /// (no matching instrument, or the family binding is unregistered — the
    /// engine logs the unregistered-family case loud). Reported, not dropped.
    NoCurrentMarket { market_key: String },
}

/// Outcome of resolving the full declared market set against a discovery
/// instrument snapshot: every market that resolved to a concrete binding, plus
/// every declared market that missed (with its reason). Misses are surfaced, not
/// silently swallowed, so an operator can see a declared market that is not
/// currently quotable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakerMarketResolution {
    pub bindings: Vec<MakerResolvedMarketBinding>,
    pub misses: Vec<MakerMarketResolutionMiss>,
}

/// Resolve a single declared market through the shared discovery engine, mapping
/// the engine's up/down instruments to the YES/NO leg bindings. Returns the
/// resolved binding, or the reason it missed. REUSES
/// [`bolt_v3_market_families::select_binary_option_market_from_target`] — no
/// discovery is reimplemented here.
pub fn resolve_declared_market(
    declaration: &MakerMarketDeclaration,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
) -> Result<MakerResolvedMarketBinding, MakerMarketResolutionMiss> {
    let Ok(cadence_seconds) = i64::try_from(declaration.cadence_seconds) else {
        return Err(MakerMarketResolutionMiss::CadenceSecondsOutOfRange {
            market_key: declaration.market_key.clone(),
            cadence_seconds: declaration.cadence_seconds,
        });
    };
    let target = MarketSelectionTarget {
        family_key: &declaration.family_key,
        underlying_asset: &declaration.underlying_asset,
        cadence_seconds,
        cadence_slug_token: &declaration.cadence_slug_token,
        static_condition_id: declaration.static_condition_id.as_deref(),
        static_yes_outcome: declaration.static_yes_outcome.as_deref(),
        static_no_outcome: declaration.static_no_outcome.as_deref(),
    };
    let Some(market) = bolt_v3_market_families::select_binary_option_market_from_target(
        target,
        instruments,
        now_milliseconds,
    ) else {
        return Err(MakerMarketResolutionMiss::NoCurrentMarket {
            market_key: declaration.market_key.clone(),
        });
    };
    Ok(MakerResolvedMarketBinding {
        market_key: declaration.market_key.clone(),
        family_key: declaration.family_key.clone(),
        market_id: market.market_id,
        yes: leg_binding(market.up_instrument_id),
        no: leg_binding(market.down_instrument_id),
        selection_outcome: market.selection_outcome,
        source_identity: market.source_identity,
        start_timestamp_milliseconds: market.start_timestamp_milliseconds,
        expiration_timestamp_milliseconds: market.expiration_timestamp_milliseconds,
        seconds_to_end: market.seconds_to_end,
    })
}

/// Resolve the full declared market set, partitioning declarations into resolved
/// bindings and misses. Each declaration is resolved independently through the
/// shared engine; a miss on one market never suppresses another.
pub fn resolve_declared_markets(
    declarations: &[MakerMarketDeclaration],
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
) -> MakerMarketResolution {
    let mut bindings = Vec::new();
    let mut misses = Vec::new();
    for declaration in declarations {
        match resolve_declared_market(declaration, instruments, now_milliseconds) {
            Ok(binding) => bindings.push(binding),
            Err(miss) => misses.push(miss),
        }
    }
    MakerMarketResolution { bindings, misses }
}

/// Build the planner candidate for a resolved market binding. A resolved binding
/// is eligible (it has concrete legs to quote); the `rotation_rank` preserves the
/// operator's declared order so the planner's deterministic fill order matches
/// declaration order. The candidate borrows the binding's `market_key`, so the
/// caller must keep the resolution alive while planning.
#[must_use]
pub fn candidate_for_binding<'a>(
    binding: &'a MakerResolvedMarketBinding,
    rotation_rank: u64,
) -> MakerMarketCandidate<'a> {
    MakerMarketCandidate {
        market_key: binding.market_key.as_str(),
        eligible: true,
        rotation_rank,
    }
}

/// Turn the resolved bindings into planner candidates and run the EXISTING
/// generic [`plan_maker_market_portfolio`] to produce a slot plan. `rotation_rank`
/// follows the order of `bindings` so the planner's fill order is the operator's
/// declared order. Does NOT reimplement the planner; this is the wiring seam from
/// resolved markets to the shared portfolio policy.
#[must_use]
pub fn plan_portfolio_from_bindings<'a>(
    policy: MakerMarketPortfolioPolicy,
    bindings: &'a [MakerResolvedMarketBinding],
    active_slots: &[MakerMarketSlotState<'a>],
) -> MakerMarketPortfolioDecision<'a> {
    let candidates: Vec<MakerMarketCandidate<'a>> = bindings
        .iter()
        .enumerate()
        .map(|(index, binding)| candidate_for_binding(binding, index as u64))
        .collect();
    plan_maker_market_portfolio(policy, &candidates, active_slots)
}

fn leg_binding(instrument_id: nautilus_model::identifiers::InstrumentId) -> MakerLegBinding {
    MakerLegBinding {
        instrument_id,
        active_order: None,
        next_order: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use nautilus_core::Params;
    use nautilus_model::{
        enums::AssetClass,
        identifiers::{InstrumentId, Symbol},
        instruments::BinaryOption,
        types::{Currency, Price, Quantity},
    };

    // `MarketSelectionOutcome` and `MarketSelectionTarget` are already in scope
    // via `super::*`; only the candidate-window helper is test-only.
    use crate::bolt_v3_market_families::market_selection_candidate_windows_from_target;

    const NOW_MS: u64 = 1_700_000_000_000;
    const ASSET: &str = "ETH";
    const CADENCE_SECONDS: u64 = 3_600;
    const CADENCE_SLUG_TOKEN: &str = "hourly";
    const UPDOWN_FAMILY: &str = "updown";

    fn declaration(market_key: &str) -> MakerMarketDeclaration {
        MakerMarketDeclaration {
            market_key: market_key.to_string(),
            family_key: UPDOWN_FAMILY.to_string(),
            underlying_asset: ASSET.to_string(),
            cadence_seconds: CADENCE_SECONDS,
            cadence_slug_token: CADENCE_SLUG_TOKEN.to_string(),
            static_condition_id: None,
            static_yes_outcome: None,
            static_no_outcome: None,
        }
    }

    /// Ask the shared engine for the exact `current` window slug and start ts it
    /// will look for, so the fixtures are derived from the engine's own contract
    /// rather than a hardcoded slug. This is what makes the resolution test a true
    /// "reuses the shared engine" assertion: if the resolver stopped calling the
    /// engine, no instrument the engine recognizes would be matched.
    fn current_window(declaration: &MakerMarketDeclaration) -> (String, u64) {
        let cadence_seconds =
            i64::try_from(declaration.cadence_seconds).expect("test cadence fits i64");
        let target = MarketSelectionTarget {
            family_key: &declaration.family_key,
            underlying_asset: &declaration.underlying_asset,
            cadence_seconds,
            cadence_slug_token: &declaration.cadence_slug_token,
            static_condition_id: None,
            static_yes_outcome: None,
            static_no_outcome: None,
        };
        let windows = market_selection_candidate_windows_from_target(target, NOW_MS)
            .expect("updown candidate windows compute for a valid target");
        let current = windows
            .into_iter()
            .find(|window| window.outcome == MarketSelectionOutcome::Current)
            .expect("a current window is always produced");
        (current.market_slug, current.start_timestamp_milliseconds)
    }

    #[allow(clippy::too_many_arguments)]
    fn test_binary_option(
        instrument_id: &str,
        market_slug: &str,
        market_id: &str,
        condition_id: &str,
        question_id: &str,
        outcome: &str,
        activation_ms: u64,
        expiration_ms: u64,
    ) -> InstrumentAny {
        let mut info = Params::new();
        for (key, value) in [
            ("market_slug", market_slug),
            ("market_id", market_id),
            ("condition_id", condition_id),
            ("question_id", question_id),
        ] {
            info.insert(
                key.to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
        InstrumentAny::BinaryOption(BinaryOption::new(
            InstrumentId::from(instrument_id),
            Symbol::from(instrument_id.split('.').next().unwrap_or(instrument_id)),
            AssetClass::Alternative,
            Currency::USDC(),
            (activation_ms.saturating_mul(1_000_000)).into(),
            (expiration_ms.saturating_mul(1_000_000)).into(),
            3,
            2,
            Price::from("0.001"),
            Quantity::from("0.01"),
            Some(ustr::Ustr::from(outcome)),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(info),
            1.into(),
            1.into(),
        ))
    }

    /// Build a selectable up/down `BinaryOption` pair for the engine's current
    /// window of `declaration`, returning `(instruments, up_id, down_id)`.
    fn selectable_pair(
        declaration: &MakerMarketDeclaration,
        up_id: &str,
        down_id: &str,
    ) -> (Vec<InstrumentAny>, InstrumentId, InstrumentId) {
        let (slug, start_ms) = current_window(declaration);
        let expiration_ms = start_ms + CADENCE_SECONDS * 1_000;
        let market_id = format!("market-{slug}");
        let condition_id = format!("condition-{slug}");
        let question_id = format!("question-{slug}");
        let up = test_binary_option(
            up_id,
            &slug,
            &market_id,
            &condition_id,
            &question_id,
            "Up",
            start_ms,
            expiration_ms,
        );
        let down = test_binary_option(
            down_id,
            &slug,
            &market_id,
            &condition_id,
            &question_id,
            "Down",
            start_ms,
            expiration_ms,
        );
        (
            vec![up, down],
            InstrumentId::from(up_id),
            InstrumentId::from(down_id),
        )
    }

    fn portfolio_policy(max_active_markets: usize) -> MakerMarketPortfolioPolicy {
        MakerMarketPortfolioPolicy {
            max_active_markets,
            total_bankroll_notional: 1_500.0,
            min_slot_notional: 100.0,
        }
    }

    #[test]
    fn declared_market_resolves_up_to_yes_and_down_to_no_via_shared_engine() {
        // Pre-fix variant: no resolver exists, so there is no path from a declared
        // market to a `MakerLegBinding`. This asserts the real side-effect channel
        // (the resolved leg bindings) and that the YES leg binds the engine's
        // up instrument and the NO leg binds the down instrument — a leg transpose
        // would flip these assertions.
        let declaration = declaration("eth-hourly");
        let (instruments, up_id, down_id) =
            selectable_pair(&declaration, "UP-OUTCOME.SIM", "DOWN-OUTCOME.SIM");

        let binding = resolve_declared_market(&declaration, &instruments, NOW_MS)
            .expect("a selectable current market must resolve");

        assert_eq!(binding.market_key, "eth-hourly");
        assert_eq!(binding.family_key, UPDOWN_FAMILY);
        assert_eq!(
            binding.yes.instrument_id, up_id,
            "the YES leg must bind the engine's up instrument"
        );
        assert_eq!(
            binding.no.instrument_id, down_id,
            "the NO leg must bind the engine's down instrument"
        );
        // PR-A does not assign order identities; PR-B does.
        assert!(binding.yes.active_order.is_none());
        assert!(binding.yes.next_order.is_none());
        assert!(binding.no.active_order.is_none());
        assert!(binding.no.next_order.is_none());
        assert_eq!(binding.selection_outcome, MarketSelectionOutcome::Current);
    }

    #[test]
    fn declared_market_with_no_current_market_reports_miss_not_silent_drop() {
        // Fail-closed: a declared market the engine cannot resolve (no matching
        // instruments) surfaces as an explicit miss, never a silent empty success.
        let declaration = declaration("eth-hourly");
        let miss = resolve_declared_market(&declaration, &[], NOW_MS)
            .expect_err("an unresolvable market must report a miss");
        assert_eq!(
            miss,
            MakerMarketResolutionMiss::NoCurrentMarket {
                market_key: "eth-hourly".to_string(),
            }
        );
    }

    #[test]
    fn resolve_declared_markets_partitions_bindings_and_misses() {
        // One resolvable market, one unresolvable: both are reported on their own
        // channel, and a miss on one never suppresses the other's binding.
        let resolvable = declaration("eth-resolvable");
        let unresolvable = MakerMarketDeclaration {
            market_key: "eth-unresolvable".to_string(),
            cadence_slug_token: "different".to_string(),
            ..declaration("eth-unresolvable")
        };
        let (instruments, up_id, _down_id) =
            selectable_pair(&resolvable, "UP-OUTCOME.SIM", "DOWN-OUTCOME.SIM");

        let resolution =
            resolve_declared_markets(&[resolvable, unresolvable], &instruments, NOW_MS);

        assert_eq!(resolution.bindings.len(), 1);
        assert_eq!(resolution.bindings[0].market_key, "eth-resolvable");
        assert_eq!(resolution.bindings[0].yes.instrument_id, up_id);
        assert_eq!(
            resolution.misses,
            vec![MakerMarketResolutionMiss::NoCurrentMarket {
                market_key: "eth-unresolvable".to_string(),
            }]
        );
    }

    #[test]
    fn resolved_candidates_feed_the_shared_portfolio_planner() {
        // Asserts the wiring seam to the EXISTING `plan_maker_market_portfolio`:
        // a resolved binding becomes an eligible candidate that the shared planner
        // turns into a retained slot with the bankroll-split allocation. A
        // reimplemented planner (not the shared one) would not produce this exact
        // slot plan shape.
        let declaration = declaration("eth-hourly");
        let (instruments, _up_id, _down_id) =
            selectable_pair(&declaration, "UP-OUTCOME.SIM", "DOWN-OUTCOME.SIM");
        let resolution = resolve_declared_markets(&[declaration], &instruments, NOW_MS);
        assert_eq!(resolution.bindings.len(), 1);

        let decision = plan_portfolio_from_bindings(portfolio_policy(3), &resolution.bindings, &[]);

        assert!(decision.blockers.is_empty(), "{:?}", decision.blockers);
        let plan = decision
            .plan
            .expect("a resolved eligible market must plan a slot");
        assert_eq!(plan.slots.len(), 1);
        assert_eq!(plan.slots[0].market_key, "eth-hourly");
        assert!(
            !plan.slots[0].retained,
            "a freshly filled slot is not retained"
        );
        // Single slot gets the full bankroll under the shared planner's even split.
        assert!((plan.slots[0].allocation_notional - 1_500.0).abs() < 1e-9);
    }

    #[test]
    fn cadence_seconds_out_of_i64_range_reports_miss_not_silent_wrap() {
        // Fail-closed numeric guard mirroring the taker's i64 conversion: a cadence
        // that cannot be represented as the engine's signed cadence is a reported
        // miss, never a silent wrap to a negative cadence.
        let declaration = MakerMarketDeclaration {
            cadence_seconds: u64::MAX,
            ..declaration("eth-hourly")
        };
        let miss = resolve_declared_market(&declaration, &[], NOW_MS)
            .expect_err("an out-of-range cadence must report a miss");
        assert_eq!(
            miss,
            MakerMarketResolutionMiss::CadenceSecondsOutOfRange {
                market_key: "eth-hourly".to_string(),
                cadence_seconds: u64::MAX,
            }
        );
    }
}
