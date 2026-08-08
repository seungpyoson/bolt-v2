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

use nautilus_model::{identifiers::InstrumentId, instruments::InstrumentAny};
use serde::Deserialize;

use crate::bolt_v3_evidence_novelty::EvidenceMarketIdentity;
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
///
/// `Deserialize` (with `deny_unknown_fields`) lets the flat NautilusTrader maker
/// config carry the operator-declared market set through to the runtime: PR-A's
/// archetype threads each declared market into the flat config table, and the
/// strategy parses it back into this type at build so `runtime::MakerRuntime` can
/// resolve markets at `on_start` without re-reading the operator `[parameters]`
/// block.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
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
    pub underlying_asset: String,
    pub market_id: String,
    pub evidence_identity: EvidenceMarketIdentity,
    pub yes: MakerLegBinding,
    pub no: MakerLegBinding,
    pub selection_outcome: MarketSelectionOutcome,
    pub source_identity: SelectedMarketSourceIdentity,
    pub start_timestamp_milliseconds: u64,
    pub expiration_timestamp_milliseconds: u64,
    pub seconds_to_end: u64,
}

/// The concrete selected market instance that owns maker throttle episodes.
///
/// The validated venue identity is shared with the registered evidence domains.
/// Window start and leg instruments additionally distinguish cadence successors
/// and venue reissues that resolve under the same configured `market_key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakerConcreteMarketIdentity {
    evidence_identity: EvidenceMarketIdentity,
    start_timestamp_milliseconds: u64,
    yes_instrument_id: InstrumentId,
    no_instrument_id: InstrumentId,
}

impl MakerConcreteMarketIdentity {
    #[must_use]
    pub fn new(
        evidence_identity: EvidenceMarketIdentity,
        start_timestamp_milliseconds: u64,
        yes_instrument_id: InstrumentId,
        no_instrument_id: InstrumentId,
    ) -> Self {
        Self {
            evidence_identity,
            start_timestamp_milliseconds,
            yes_instrument_id,
            no_instrument_id,
        }
    }

    #[must_use]
    pub fn gamma_market_id(&self) -> &str {
        self.evidence_identity.gamma_market_id()
    }

    /// The validated venue identity, excluding cadence and internal instrument
    /// discriminators that complete this concrete maker-market identity.
    #[must_use]
    pub fn evidence_identity(&self) -> &EvidenceMarketIdentity {
        &self.evidence_identity
    }
}

impl MakerResolvedMarketBinding {
    #[must_use]
    pub fn concrete_identity(&self) -> MakerConcreteMarketIdentity {
        MakerConcreteMarketIdentity::new(
            self.evidence_identity.clone(),
            self.start_timestamp_milliseconds,
            self.yes.instrument_id,
            self.no.instrument_id,
        )
    }
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
    let evidence_identity = market.evidence_identity.market().clone();
    Ok(MakerResolvedMarketBinding {
        market_key: declaration.market_key.clone(),
        family_key: declaration.family_key.clone(),
        underlying_asset: declaration.underlying_asset.clone(),
        market_id: market.market_id,
        evidence_identity,
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
    const CADENCE_SLUG_TOKEN: &str = "1h";
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
        // Production instruments carry this -- the pinned adapter always writes
        // it (`http/parse.rs`), so a fixture without it is not a real
        // instrument.
        info.insert("neg_risk".to_string(), serde_json::Value::Bool(false));
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

    // --- static_binary_event maker resolution (PR #822 review disproof) ---
    // An external review (GPT) raised a HIGH claim that the static_binary_event
    // maker target reuses the taker's `event_key -> underlying_asset` projection
    // (static_binary_event::target_runtime_fields), so a "canonical" static market
    // whose identity is a lowercase event key would fail the maker load gate
    // (which validates underlying_asset with the uppercase ASSET rule) and only an
    // asset symbol like ETH would pass. That projection is taker-only: the maker
    // binding has NO event_key field, and the static family selects a market
    // strictly by market_slug + condition_id + yes/no outcomes
    // (select_binary_option_market) and never reads underlying_asset. The test
    // below pins that contract: a static market declared with an uppercase asset
    // resolves the intended YES/NO pair, and re-resolving the SAME instruments with
    // a DIFFERENT asset resolves the IDENTICAL market. If underlying_asset were a
    // static selection key (i.e. had to carry the event key), the second
    // resolution would miss. (Load-gate acceptance of this shape is pinned
    // separately by
    // archetype::validate_parameter_bounds_accepts_valid_static_binary_event_declaration.)

    const STATIC_FAMILY: &str = "static_binary_event";
    const STATIC_SLUG: &str = "will-sample-event-resolve-yes";
    const STATIC_CONDITION_ID: &str = "condition-sample-event";
    const STATIC_YES_OUTCOME: &str = "Yes";
    const STATIC_NO_OUTCOME: &str = "No";

    fn static_declaration(market_key: &str, underlying_asset: &str) -> MakerMarketDeclaration {
        MakerMarketDeclaration {
            market_key: market_key.to_string(),
            family_key: STATIC_FAMILY.to_string(),
            underlying_asset: underlying_asset.to_string(),
            cadence_seconds: CADENCE_SECONDS,
            cadence_slug_token: STATIC_SLUG.to_string(),
            static_condition_id: Some(STATIC_CONDITION_ID.to_string()),
            static_yes_outcome: Some(STATIC_YES_OUTCOME.to_string()),
            static_no_outcome: Some(STATIC_NO_OUTCOME.to_string()),
        }
    }

    /// Build a selectable YES/NO static pair matching `declaration`'s slug +
    /// condition + outcomes. Deliberately does NOT consult `underlying_asset` — the
    /// static family identifies a market without it.
    fn static_selectable_pair(
        declaration: &MakerMarketDeclaration,
        yes_id: &str,
        no_id: &str,
    ) -> Vec<InstrumentAny> {
        let slug = declaration.cadence_slug_token.as_str();
        let condition_id = declaration
            .static_condition_id
            .as_deref()
            .expect("static declaration carries a condition id");
        let yes_outcome = declaration
            .static_yes_outcome
            .as_deref()
            .expect("static declaration carries a yes outcome");
        let no_outcome = declaration
            .static_no_outcome
            .as_deref()
            .expect("static declaration carries a no outcome");
        let market_id = format!("market-{slug}");
        let question_id = format!("question-{slug}");
        let activation_ms = NOW_MS - 1_000;
        let expiration_ms = NOW_MS + 30_000;
        vec![
            test_binary_option(
                yes_id,
                slug,
                &market_id,
                condition_id,
                &question_id,
                yes_outcome,
                activation_ms,
                expiration_ms,
            ),
            test_binary_option(
                no_id,
                slug,
                &market_id,
                condition_id,
                &question_id,
                no_outcome,
                activation_ms,
                expiration_ms,
            ),
        ]
    }

    #[test]
    fn static_market_resolves_and_is_invariant_to_underlying_asset() {
        let eth = static_declaration("sample-event-eth", "ETH");
        let instruments =
            static_selectable_pair(&eth, "SAMPLE-EVENT-YES.SIM", "SAMPLE-EVENT-NO.SIM");

        let eth_binding = resolve_declared_market(&eth, &instruments, NOW_MS)
            .expect("a static market declared with an uppercase asset must resolve");
        assert_eq!(
            eth_binding.yes.instrument_id,
            InstrumentId::from("SAMPLE-EVENT-YES.SIM"),
            "the YES leg must bind the configured yes-outcome instrument"
        );
        assert_eq!(
            eth_binding.no.instrument_id,
            InstrumentId::from("SAMPLE-EVENT-NO.SIM"),
            "the NO leg must bind the configured no-outcome instrument"
        );

        // Same market identity (slug/condition/outcomes), different underlying_asset:
        // the IDENTICAL market resolves, proving underlying_asset is not a static
        // selection key and so need not (and does not) carry the event key.
        let btc = static_declaration("sample-event-btc", "BTC");
        let btc_binding = resolve_declared_market(&btc, &instruments, NOW_MS).expect(
            "underlying_asset is not a static selection key; a different asset still resolves",
        );
        assert_eq!(btc_binding.market_id, eth_binding.market_id);
        assert_eq!(btc_binding.yes.instrument_id, eth_binding.yes.instrument_id);
        assert_eq!(btc_binding.no.instrument_id, eth_binding.no.instrument_id);
    }
}
