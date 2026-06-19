//! Maker runtime foundation (PR-B, #817 / umbrella #488).
//!
//! Consumes the per-market bindings PR-A resolves
//! ([`super::binding::resolve_declared_markets`]) and owns the maker's *outbound*
//! runtime state: which declared markets are active each cycle (via the existing
//! shared portfolio planner), the per-leg order identities the order plan assigns,
//! and the rotation those identities follow as quotes are placed and cancelled.
//!
//! It owns **no NautilusTrader types**: the strategy shell (`mod.rs`) bridges NT
//! cache/clock/subscription calls into these pure operations, so the whole runtime
//! is exhaustively unit/integration-testable without a node. **INTENT ONLY** —
//! nothing here submits to a venue; order routing flows through the global
//! execution-policy chokepoint (`bolt_v3_order_execution`), which suppresses every
//! venue mutation while `runtime.order_execution_mode = shadow` (the default until
//! the maker is armed).
//!
//! Deferred to later #817 slices (NOT this PR): the `MarketQuote` two-leg
//! cancel-scope controller + quote math (X2), the fair-value / realized-volatility
//! feed threading that makes a quote cycle produce a non-blocked fair value, the
//! settlement trigger (X3), the active-flatten governor (X4), the μ/fee economics
//! (X5), and the inbound NT order-event → lifecycle reconciliation. Until the
//! fair-value feed lands, a live quote cycle fails closed (no fair value → no
//! intent); the identity-assignment and rotation machinery here is exercised by
//! the differential tests, which inject a ready quote input.

use std::collections::{BTreeMap, BTreeSet};

use nautilus_model::{identifiers::InstrumentId, instruments::InstrumentAny};

use crate::{
    bolt_v3_maker_event_fence::{ClientOrderId as MakerClientOrderId, OrderIdentity},
    bolt_v3_maker_market_selection::{
        MakerMarketPortfolioPolicy, MakerMarketSlotState, MakerPerMarketHealth, MakerPerMarketKill,
    },
    bolt_v3_maker_order_dispatch::MakerOrderDispatchOutcome,
    bolt_v3_maker_order_plan::MakerLegBinding,
    bolt_v3_maker_runtime_order::MakerRuntimeOrderDispatchOutcome,
    bolt_v3_maker_runtime_quote::MakerRuntimeOrderPlanInput,
    bolt_v3_quote_lifecycle::Leg,
};

use super::binding::{
    MakerMarketDeclaration, MakerMarketResolution, MakerMarketResolutionMiss,
    MakerResolvedMarketBinding, plan_portfolio_from_bindings, resolve_declared_markets,
};

/// The stable per-leg order-identity tag a minted [`OrderIdentity`]'s client order
/// id carries, so a YES and a NO order of the same generation never collide.
fn leg_tag(leg: Leg) -> &'static str {
    match leg {
        Leg::Yes => "yes",
        Leg::No => "no",
    }
}

/// Mint a fresh per-leg [`OrderIdentity`] for a market generation. The client
/// order id is `"{order_id_tag}-{market_key}-{yes|no}-{generation}"`, which the
/// order compiler maps **verbatim** onto the NautilusTrader `ClientOrderId`
/// (`bolt_v3_maker_order_compile::nt_client_order_id`), so it must be unique per
/// live order: `order_id_tag` is unique per strategy, `market_key` per declared
/// market, `leg` distinguishes the two legs, and `generation` is monotonic per
/// (market, leg) — together globally unique.
fn make_leg_identity(
    order_id_tag: &str,
    market_key: &str,
    leg: Leg,
    generation: u64,
) -> OrderIdentity {
    OrderIdentity::new(
        MakerClientOrderId::new(format!(
            "{order_id_tag}-{market_key}-{}-{generation}",
            leg_tag(leg)
        )),
        generation,
    )
}

/// Per-active-market runtime state. Holds the resolved binding (whose `yes`/`no`
/// [`MakerLegBinding`]s carry the order identities PR-B assigns), the monotonic
/// per-leg generation counters used to mint fresh identities, and the bankroll
/// slice the portfolio planner allocated this market.
#[derive(Debug, Clone, PartialEq)]
pub struct MakerMarketRuntime {
    binding: MakerResolvedMarketBinding,
    yes_generation: u64,
    no_generation: u64,
    allocation_notional: f64,
}

impl MakerMarketRuntime {
    fn new(binding: MakerResolvedMarketBinding, allocation_notional: f64) -> Self {
        Self {
            binding,
            yes_generation: 0,
            no_generation: 0,
            allocation_notional,
        }
    }

    /// The operator-stable market key the portfolio planner keys this slot by.
    #[must_use]
    pub fn market_key(&self) -> &str {
        &self.binding.market_key
    }

    /// The concrete current market id this slot resolved to. A different id for the
    /// same `market_key` means the cadence window rolled, so a refresh treats it as
    /// a fresh market (identities reset) rather than retaining stale ones.
    #[must_use]
    pub fn market_id(&self) -> &str {
        &self.binding.market_id
    }

    /// The bankroll notional the planner allocated this market this cycle.
    #[must_use]
    pub fn allocation_notional(&self) -> f64 {
        self.allocation_notional
    }

    /// The current leg binding (instrument id + assigned `active`/`next` order
    /// identities) for one leg.
    #[must_use]
    pub fn leg_binding(&self, leg: Leg) -> &MakerLegBinding {
        match leg {
            Leg::Yes => &self.binding.yes,
            Leg::No => &self.binding.no,
        }
    }

    /// The order-plan input the runtime quote cycle feeds to
    /// `route_maker_runtime_quote` — the current yes/no leg bindings with their
    /// assigned identities. Cloned because the route call consumes it by value.
    #[must_use]
    pub fn order_plan_input(&self) -> MakerRuntimeOrderPlanInput {
        MakerRuntimeOrderPlanInput {
            yes: self.binding.yes.clone(),
            no: self.binding.no.clone(),
        }
    }

    fn leg_binding_mut(&mut self, leg: Leg) -> &mut MakerLegBinding {
        match leg {
            Leg::Yes => &mut self.binding.yes,
            Leg::No => &mut self.binding.no,
        }
    }

    fn mint_next(&mut self, order_id_tag: &str, leg: Leg) {
        let generation = match leg {
            Leg::Yes => {
                self.yes_generation += 1;
                self.yes_generation
            }
            Leg::No => {
                self.no_generation += 1;
                self.no_generation
            }
        };
        let market_key = self.binding.market_key.clone();
        self.leg_binding_mut(leg).next_order = Some(make_leg_identity(
            order_id_tag,
            &market_key,
            leg,
            generation,
        ));
    }
}

/// The subscription delta and per-market resolution misses produced by one
/// [`MakerRuntime::refresh_active_markets`] pass. The shell subscribes every
/// `subscribe` instrument's trade feed and unsubscribes every `unsubscribe` one;
/// `misses` surface declared markets that did not resolve (never silently
/// dropped), mirroring PR-A's fail-closed resolution contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakerRuntimeRefresh {
    pub subscribe: Vec<InstrumentId>,
    pub unsubscribe: Vec<InstrumentId>,
    pub misses: Vec<MakerMarketResolutionMiss>,
}

/// Owns the maker's per-market runtime state across the active market set. Pure:
/// it resolves declared markets against a caller-supplied instrument snapshot,
/// runs the shared portfolio planner to pick the active set, and tracks the order
/// identities each active market's legs hold. The NT shell drives it from
/// `on_start` / `on_time_event`.
#[derive(Debug, Clone, PartialEq)]
pub struct MakerRuntime {
    markets: BTreeMap<String, MakerMarketRuntime>,
}

impl MakerRuntime {
    /// A runtime with no active markets. Named `empty` rather than `new` because
    /// the bolt-v3 legacy-default fence forbids a `Default` impl on the production
    /// surface, so a no-argument `new` would trip `clippy::new_without_default`
    /// with no sanctioned way to satisfy it; an explicit named constructor is the
    /// repo idiom.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            markets: BTreeMap::new(),
        }
    }

    /// Whether any market is currently active.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.markets.is_empty()
    }

    /// The number of currently active markets.
    #[must_use]
    pub fn active_market_count(&self) -> usize {
        self.markets.len()
    }

    /// The runtime state for one active market, if it is active.
    #[must_use]
    pub fn market(&self, market_key: &str) -> Option<&MakerMarketRuntime> {
        self.markets.get(market_key)
    }

    /// Every active market's leg instrument ids, sorted and de-duplicated — the
    /// instruments the shell keeps a trade subscription open for.
    #[must_use]
    pub fn active_instrument_ids(&self) -> Vec<InstrumentId> {
        self.instrument_id_set().into_iter().collect()
    }

    fn instrument_id_set(&self) -> BTreeSet<InstrumentId> {
        self.markets
            .values()
            .flat_map(|market| {
                [
                    market.binding.yes.instrument_id,
                    market.binding.no.instrument_id,
                ]
            })
            .collect()
    }

    /// Resolve the declared market set against the current instrument snapshot,
    /// re-plan the active portfolio, and reconcile per-market runtime state — the
    /// `on_start` / `on_time_event` entry point. Markets whose cadence window is
    /// unchanged (same resolved `market_id`) retain their assigned order identities
    /// and generation counters; rolled or newly-filled markets start fresh.
    /// Returns the trade-subscription delta plus any resolution misses.
    pub fn refresh_active_markets(
        &mut self,
        declarations: &[MakerMarketDeclaration],
        instruments: &[InstrumentAny],
        now_milliseconds: u64,
        policy: MakerMarketPortfolioPolicy,
    ) -> MakerRuntimeRefresh {
        let resolution = resolve_declared_markets(declarations, instruments, now_milliseconds);
        self.apply_resolution(resolution, policy)
    }

    fn apply_resolution(
        &mut self,
        resolution: MakerMarketResolution,
        policy: MakerMarketPortfolioPolicy,
    ) -> MakerRuntimeRefresh {
        let before = self.instrument_id_set();

        // Carry the current active set forward as healthy/clear slots so the shared
        // planner retains them when still discoverable. Per-market health/kill
        // predicates are a later slice (X4); the foundation treats every active
        // market as healthy and clear.
        let active_keys: Vec<String> = self.markets.keys().cloned().collect();
        let active_slots: Vec<MakerMarketSlotState> = active_keys
            .iter()
            .map(|market_key| MakerMarketSlotState {
                market_key: market_key.as_str(),
                health: MakerPerMarketHealth::Healthy,
                kill: MakerPerMarketKill::Clear,
            })
            .collect();

        let planned: Vec<(String, f64)> =
            match plan_portfolio_from_bindings(policy, &resolution.bindings, &active_slots).plan {
                // No eligible markets (empty/blocked candidate set) drops the active
                // set to empty — an explicit `Vec::new()` rather than `unwrap_or_default`
                // (the bolt-v3 legacy-default fence forbids the latter on the production
                // surface).
                None => Vec::new(),
                Some(plan) => plan
                    .slots
                    .into_iter()
                    .map(|slot| (slot.market_key.to_string(), slot.allocation_notional))
                    .collect(),
            };

        let mut next: BTreeMap<String, MakerMarketRuntime> = BTreeMap::new();
        for (market_key, allocation_notional) in planned {
            let Some(binding) = resolution
                .bindings
                .iter()
                .find(|binding| binding.market_key == market_key)
                .cloned()
            else {
                // The planner only ever returns keys present in `bindings`; this is
                // a defensive guard, never expected to fire.
                continue;
            };
            let runtime = match self.markets.remove(&market_key) {
                // Same cadence window: retain assigned identities + generations.
                Some(mut prior) if prior.binding.market_id == binding.market_id => {
                    prior.allocation_notional = allocation_notional;
                    prior
                }
                // New or rolled window: fresh identities.
                _ => MakerMarketRuntime::new(binding, allocation_notional),
            };
            next.insert(market_key, runtime);
        }
        self.markets = next;

        let after = self.instrument_id_set();
        MakerRuntimeRefresh {
            subscribe: after.difference(&before).copied().collect(),
            unsubscribe: before.difference(&after).copied().collect(),
            misses: resolution.misses,
        }
    }

    /// Mint a fresh `next_order` identity on both legs of an active market ahead of
    /// a quote cycle, so the order plan can produce a Submit intent (the plan fails
    /// closed with `MissingNextOrderIdentity` if `next_order` is unset). Returns
    /// `false` if the market is not active. The generation is monotonic per
    /// (market, leg), so re-minting before a prior intent is dispatched simply
    /// supersedes it.
    pub fn mint_next_identities(&mut self, market_key: &str, order_id_tag: &str) -> bool {
        let Some(market) = self.markets.get_mut(market_key) else {
            return false;
        };
        market.mint_next(order_id_tag, Leg::Yes);
        market.mint_next(order_id_tag, Leg::No);
        true
    }

    /// Rotate each leg's order identities from the result of a dispatched quote
    /// cycle: a Submit promotes `next_order` to `active_order` (the order now rests
    /// from the maker's outbound view), a Cancel / CancelAll clears `active_order`,
    /// and a Modify leaves the resting identity in place. A leg with no dispatch
    /// (blocked or no action) is untouched. Returns `false` if the market is not
    /// active.
    pub fn apply_dispatch_outcome(
        &mut self,
        market_key: &str,
        outcome: &MakerRuntimeOrderDispatchOutcome,
    ) -> bool {
        let Some(market) = self.markets.get_mut(market_key) else {
            return false;
        };
        rotate_leg_identity(
            market.leg_binding_mut(Leg::Yes),
            outcome.yes.dispatch.as_ref(),
        );
        rotate_leg_identity(
            market.leg_binding_mut(Leg::No),
            outcome.no.dispatch.as_ref(),
        );
        true
    }
}

/// Apply one leg's dispatched intent to its identity slots. See
/// [`MakerRuntime::apply_dispatch_outcome`].
fn rotate_leg_identity(
    binding: &mut MakerLegBinding,
    dispatch: Option<&MakerOrderDispatchOutcome>,
) {
    match dispatch {
        Some(MakerOrderDispatchOutcome::Submitted { .. }) => {
            binding.active_order = binding.next_order.take();
        }
        Some(
            MakerOrderDispatchOutcome::Canceled { .. }
            | MakerOrderDispatchOutcome::CanceledAll { .. },
        ) => {
            binding.active_order = None;
        }
        // A modify amends the resting order in place: the active identity is
        // unchanged. No dispatch (a blocked or no-action leg) leaves the slots as-is.
        Some(MakerOrderDispatchOutcome::Modified { .. }) | None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use nautilus_model::identifiers::{ClientOrderId, InstrumentId};
    use nautilus_model::{
        enums::OrderSide,
        types::{Price, Quantity},
    };

    fn leg_binding(instrument: &str) -> MakerLegBinding {
        MakerLegBinding {
            instrument_id: InstrumentId::from(instrument),
            active_order: None,
            next_order: None,
        }
    }

    #[test]
    fn minted_identities_are_deterministic_and_per_leg_unique() {
        // The client order id is the cancel/modify handle the venue knows, so a YES
        // and a NO order of the same generation must never collide, and the same
        // (tag, market, leg, generation) must reproduce the same id. A leg_tag
        // collision (e.g. both legs minting "yes") would make the YES and NO ids
        // equal and fail the inequality.
        let yes = make_leg_identity("001", "eth-hourly", Leg::Yes, 3);
        let no = make_leg_identity("001", "eth-hourly", Leg::No, 3);
        assert_eq!(yes.client_order_id().as_str(), "001-eth-hourly-yes-3");
        assert_eq!(no.client_order_id().as_str(), "001-eth-hourly-no-3");
        assert_ne!(yes.client_order_id(), no.client_order_id());
        assert_eq!(yes.generation(), 3);
        assert_eq!(
            make_leg_identity("001", "eth-hourly", Leg::Yes, 3),
            yes,
            "minting is a pure function of its inputs"
        );
    }

    #[test]
    fn submit_dispatch_promotes_next_identity_to_active() {
        // Differential rotation guard: a Submit promotes next_order -> active_order
        // (the order now rests, so a later requote cancels it via active_order). A
        // no-op rotation would leave active_order None, so the assertion fails.
        let mut binding = leg_binding("YES.SIM");
        let minted = make_leg_identity("001", "m", Leg::Yes, 1);
        binding.next_order = Some(minted.clone());
        rotate_leg_identity(
            &mut binding,
            Some(&MakerOrderDispatchOutcome::Submitted {
                leg: Leg::Yes,
                instrument_id: InstrumentId::from("YES.SIM"),
                client_order_id: ClientOrderId::from("001-m-yes-1"),
                price: Price::new(0.40, 2),
                quantity: Quantity::new(1.0, 0),
            }),
        );
        assert_eq!(binding.active_order, Some(minted));
        assert_eq!(
            binding.next_order, None,
            "the promoted identity leaves next"
        );
    }

    #[test]
    fn cancel_dispatch_clears_active_identity() {
        // A wind-down/requote cancel clears the resting identity; a no-op rotation
        // would strand active_order set, so the next cancel would target a gone order.
        let mut binding = leg_binding("NO.SIM");
        binding.active_order = Some(make_leg_identity("001", "m", Leg::No, 2));
        rotate_leg_identity(
            &mut binding,
            Some(&MakerOrderDispatchOutcome::Canceled {
                leg: Leg::No,
                instrument_id: InstrumentId::from("NO.SIM"),
                client_order_id: ClientOrderId::from("001-m-no-2"),
            }),
        );
        assert_eq!(binding.active_order, None);
    }

    #[test]
    fn cancel_all_dispatch_clears_active_identity() {
        let mut binding = leg_binding("YES.SIM");
        binding.active_order = Some(make_leg_identity("001", "m", Leg::Yes, 5));
        rotate_leg_identity(
            &mut binding,
            Some(&MakerOrderDispatchOutcome::CanceledAll {
                leg: Some(Leg::Yes),
                instrument_id: InstrumentId::from("YES.SIM"),
                order_side: Some(OrderSide::Buy),
            }),
        );
        assert_eq!(binding.active_order, None);
    }

    #[test]
    fn modify_dispatch_keeps_active_identity() {
        // A modify amends in place: the resting identity must survive, otherwise a
        // later cancel could not target it.
        let mut binding = leg_binding("YES.SIM");
        let resting = make_leg_identity("001", "m", Leg::Yes, 4);
        binding.active_order = Some(resting.clone());
        rotate_leg_identity(
            &mut binding,
            Some(&MakerOrderDispatchOutcome::Modified {
                leg: Leg::Yes,
                instrument_id: InstrumentId::from("YES.SIM"),
                client_order_id: ClientOrderId::from("001-m-yes-4"),
                price: Price::new(0.42, 2),
                quantity: Quantity::new(1.0, 0),
            }),
        );
        assert_eq!(binding.active_order, Some(resting));
    }

    #[test]
    fn blocked_leg_leaves_identities_untouched() {
        let mut binding = leg_binding("NO.SIM");
        let next = make_leg_identity("001", "m", Leg::No, 1);
        binding.next_order = Some(next.clone());
        rotate_leg_identity(&mut binding, None);
        assert_eq!(binding.next_order, Some(next));
        assert_eq!(binding.active_order, None);
    }
}
