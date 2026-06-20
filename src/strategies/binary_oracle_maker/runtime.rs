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
/// order id is
/// `"{order_id_tag}-{market_key}-{window_start_milliseconds}-{yes|no}-{generation}"`,
/// which the order compiler maps **verbatim** onto the NautilusTrader
/// `ClientOrderId` (`bolt_v3_maker_order_compile::nt_client_order_id`), so it must
/// be unique per live order: `order_id_tag` is unique per strategy, `market_key`
/// per declared market, `leg` distinguishes the two legs, and `generation` is the
/// monotonic per-(market_key, leg) counter that **guarantees** uniqueness for the
/// whole in-process strategy lifetime. The high-water generation lives on
/// [`MakerRuntime`] keyed by `market_key` ([`MakerRuntime::generations`]) — not on
/// the per-refresh [`MakerMarketRuntime`] — so it **survives a market dropping out
/// of the active set**, and is the seed every rebuilt per-market runtime starts
/// from ([`MakerRuntime::apply_resolution`]). No transition can re-mint a client
/// order id a prior generation already consumed (NautilusTrader never reuses a
/// `ClientOrderId`): not a new cadence window, not a re-issued leg instrument under
/// an unchanged window, not a venue `market_id` reuse, and not a market that goes
/// inactive (planner block / transient resolution miss / `on_stop` deactivation)
/// and later refills the SAME window — the counter is never reset to 0 once a
/// (market_key, leg) has minted. (Durability across a full *process* restart needs
/// a persisted high-water; that is arming-time work, tracked in #869.)
/// `window_start_milliseconds` is the resolved cadence window's start
/// (`MakerResolvedMarketBinding::start_timestamp_milliseconds`); it and `leg` keep the
/// id human-readable and the source tuple recoverable, but uniqueness rests on the
/// monotonic `generation`, not on the window start changing (an instrument-only roll,
/// or a same-window refill, leaves it unchanged). The venue `market_id` is
/// deliberately **not** a component: it is venue metadata not guaranteed to change
/// per window, so keying the id (or the retain-vs-roll decision) on it would reopen
/// the collision class on a `market_id`-reuse roll. The positional encoding is
/// unambiguous because `order_id_tag` is rejected at load when it contains the
/// delimiter — on the live node-startup path by the bounds gate
/// (`archetype::validate_strategy`, which the node actually runs) and on the
/// builder `validate_config` path by `config::validate_order_id_tag_delimiter_free`
/// — while `window_start_milliseconds` and `generation` are decimal and `leg` is
/// `yes`/`no`, so the source tuple is recoverable from the string even when
/// `market_key` contains the delimiter.
fn make_leg_identity(
    order_id_tag: &str,
    market_key: &str,
    window_start_milliseconds: u64,
    leg: Leg,
    generation: u64,
) -> OrderIdentity {
    OrderIdentity::new(
        MakerClientOrderId::new(format!(
            "{order_id_tag}-{market_key}-{window_start_milliseconds}-{}-{generation}",
            leg_tag(leg)
        )),
        generation,
    )
}

/// The per-leg generation high-water marks for one declared market — the highest
/// generation its YES and NO legs have minted. Held by [`MakerRuntime`] keyed by
/// `market_key` (not by the per-refresh [`MakerMarketRuntime`]) so the next
/// generation a (market_key, leg) mints survives the market dropping out of the
/// active set and refilling. `Copy` so it threads through the rebuild seed by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LegGenerations {
    yes: u64,
    no: u64,
}

impl LegGenerations {
    /// The seed for a (market_key, leg) that has never minted. An explicit named
    /// constant rather than a `Default` impl: the bolt-v3 legacy-default fence
    /// forbids `Default` on the production surface.
    const ZERO: Self = Self { yes: 0, no: 0 };
}

/// Per-active-market runtime state. Holds the resolved binding (whose `yes`/`no`
/// [`MakerLegBinding`]s carry the order identities PR-B assigns), the per-leg
/// generation counters used to mint fresh identities (seeded from the persistent
/// [`MakerRuntime`] high-water so they stay monotonic across a drop/refill, never
/// resetting to 0), and the bankroll slice the portfolio planner allocated this
/// market.
#[derive(Debug, Clone, PartialEq)]
pub struct MakerMarketRuntime {
    binding: MakerResolvedMarketBinding,
    yes_generation: u64,
    no_generation: u64,
    allocation_notional: f64,
}

impl MakerMarketRuntime {
    /// Build a per-market runtime, **seeding the per-leg generation counters from
    /// the persistent [`MakerRuntime`] high-water** for this `market_key`. A
    /// brand-new (market_key, leg) seeds at [`LegGenerations::ZERO`]; a roll or a
    /// drop/refill of a market that already minted seeds at the high-water the prior
    /// active period reached, so a re-mint never reproduces a `ClientOrderId` a
    /// prior generation consumed — the id embeds `generation` but not the instrument
    /// id, and an instrument-only roll or a same-window refill leaves the embedded
    /// window start unchanged. See [`MakerRuntime::apply_resolution`].
    fn seeded(
        binding: MakerResolvedMarketBinding,
        allocation_notional: f64,
        generations: LegGenerations,
    ) -> Self {
        Self {
            binding,
            yes_generation: generations.yes,
            no_generation: generations.no,
            allocation_notional,
        }
    }

    /// The current per-leg generation counters, captured into the [`MakerRuntime`]
    /// high-water after each mint so they survive this runtime being dropped.
    fn leg_generations(&self) -> LegGenerations {
        LegGenerations {
            yes: self.yes_generation,
            no: self.no_generation,
        }
    }

    /// The operator-stable market key the portfolio planner keys this slot by.
    #[must_use]
    pub fn market_key(&self) -> &str {
        &self.binding.market_key
    }

    /// The concrete venue `market_id` this slot resolved to. Retained as a plain
    /// diagnostics accessor; it is **not** the cadence-window discriminator (that is
    /// `start_timestamp_milliseconds`), because venue metadata is not guaranteed to
    /// change when the window rolls.
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
        // `generation` is a monotonic `u64`; the increment is unchecked because
        // exhausting it needs ~1.8e19 mints of a single (market_key, leg) (~3e8 years at
        // a 1 ms cadence) AND there is no live mint driver at this foundation. Checked
        // fail-loud arithmetic and the durable cross-process high-water both belong to the
        // arming-time generation rework (#869); adding a fallible mint here would ripple
        // signatures the rework redoes. (Debug builds already panic on overflow.)
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
        let identity = make_leg_identity(
            order_id_tag,
            &self.binding.market_key,
            self.binding.start_timestamp_milliseconds,
            leg,
            generation,
        );
        self.leg_binding_mut(leg).next_order = Some(identity);
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
    /// Per-(market_key, leg) generation high-water marks — the single source of
    /// truth for the next generation each leg mints. Persists independently of
    /// `markets`: a market that drops out of the active set (planner block,
    /// transient resolution miss, `on_stop` deactivation) leaves its entry here, so
    /// a refill in the SAME cadence window re-seeds from the high-water instead of 0
    /// and never re-mints a consumed `ClientOrderId`. Never pruned within a process
    /// (bounded by the operator-declared market count); pruning would reopen the
    /// collision class. Updated on every mint, read on every rebuild.
    generations: BTreeMap<String, LegGenerations>,
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
            generations: BTreeMap::new(),
        }
    }

    /// Drop every active market — clearing the trade-subscription set so a restart
    /// re-emits the full subscribe delta — while **retaining the per-(market_key,
    /// leg) generation high-water marks**. `on_stop` uses this instead of replacing
    /// the runtime with [`MakerRuntime::empty`], so a within-process stop/start (or
    /// any drop/refill) cannot re-mint a `ClientOrderId` a prior active period
    /// already consumed. Durability across a full process restart still needs a
    /// persisted high-water (arming-time work, #869).
    pub fn deactivate_all(&mut self) {
        self.markets = BTreeMap::new();
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
    /// unchanged (same resolved `start_timestamp_milliseconds` and leg instruments)
    /// retain their assigned order identities; every rebuilt market — a roll, or a
    /// (re)fill of a market that was inactive — seeds its generation counters from
    /// the persistent per-(market_key, leg) high-water ([`MakerRuntime::generations`]),
    /// so re-minted ids stay unique across a roll AND across a drop/refill. Returns
    /// the trade-subscription delta plus any resolution misses.
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
        let active_slots: Vec<MakerMarketSlotState> = self
            .markets
            .keys()
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
            else {
                // The planner only ever returns keys present in `bindings`; this is
                // a defensive guard, never expected to fire.
                continue;
            };
            // The persistent generation high-water for this market_key. A market that
            // was active and minted leaves its high-water here even after it drops out
            // of `self.markets`, so every rebuild below seeds from the last generation
            // consumed, never 0. Read before the `remove` borrow so the two field
            // borrows don't overlap.
            let seed = self
                .generations
                .get(&market_key)
                .copied()
                .unwrap_or(LegGenerations::ZERO);
            let runtime = match self.markets.remove(&market_key) {
                // Same cadence window AND same resolved leg instruments (`same_window`
                // compares both): retain assigned identities + live counters and refresh
                // only the allocation. A changed leg instrument under an unchanged window
                // start is NOT retained — `same_window` treats it as a roll, so the live
                // trade-subscription differ never strands on a re-issued instrument. The
                // prior binding's remaining metadata (expiration, selection outcome) IS
                // intentionally left un-refreshed here: no consumer reads those fields
                // until settlement, so a stale value is latent at the foundation. The X3
                // settlement slice refreshes that metadata on retain — tracked in #866.
                Some(mut prior) if same_window(&prior.binding, binding) => {
                    prior.allocation_notional = allocation_notional;
                    prior
                }
                // Roll of a still-active market (a new window start, OR a re-issued leg
                // instrument at an unchanged window), OR a (re)fill of a market that was
                // inactive (planner block / transient resolution miss / `on_stop`):
                // rebuild the binding fresh but SEED the per-leg generation counters from
                // the persistent high-water. The client order id embeds `generation` but
                // not the instrument id, and an instrument-only roll or a same-window
                // refill leaves `start_timestamp_milliseconds` unchanged, so seeding at 0
                // would re-mint a client order id a prior generation already consumed
                // (NautilusTrader never reuses a `ClientOrderId`). Seeding from the
                // high-water keeps every minted id unique regardless of transition kind.
                // Clone the binding only here, never on the common retain path.
                _ => MakerMarketRuntime::seeded(binding.clone(), allocation_notional, seed),
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
    /// supersedes it. Each mint also advances the persistent per-(market_key, leg)
    /// high-water ([`MakerRuntime::generations`]), so the counter survives the market
    /// dropping out of the active set and a same-window refill never repeats an id.
    pub fn mint_next_identities(&mut self, market_key: &str, order_id_tag: &str) -> bool {
        let Some(market) = self.markets.get_mut(market_key) else {
            return false;
        };
        market.mint_next(order_id_tag, Leg::Yes);
        market.mint_next(order_id_tag, Leg::No);
        // Record the advanced counters into the persistent high-water so they survive
        // this market dropping out of the active set: a same-window refill then
        // re-seeds from here instead of 0 (see `MakerRuntime::generations`).
        let generations = market.leg_generations();
        self.generations.insert(market_key.to_string(), generations);
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

/// Whether two bindings of the same declared market describe the same cadence
/// window *and* resolved to the same leg instruments. The window start
/// (`start_timestamp_milliseconds`) — not the venue `market_id`, which is metadata
/// not guaranteed to change on a roll — is the primary discriminator, so a genuine
/// roll (a new `start_timestamp_milliseconds`) always rebuilds the runtime with
/// fresh identities even when the venue reuses the same `market_id`. The leg
/// instrument ids are compared too, so a venue that re-issues the period's market
/// under new instrument ids at an unchanged window start is treated as a roll
/// (fail-closed): the leg instrument id is read live by the trade-subscription
/// differ ([`MakerRuntime::instrument_id_set`]), so a window-start-only retain would
/// strand the maker on the gone instrument's feed. The discovery engine resolves a
/// period to stable instruments, so in practice the instrument ids match whenever
/// the window start does; comparing them makes that trusted invariant fail-closed
/// rather than merely assumed.
fn same_window(prior: &MakerResolvedMarketBinding, current: &MakerResolvedMarketBinding) -> bool {
    prior.start_timestamp_milliseconds == current.start_timestamp_milliseconds
        && prior.yes.instrument_id == current.yes.instrument_id
        && prior.no.instrument_id == current.no.instrument_id
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
            // A cancel/drain clears the resting identity AND drops the pre-minted
            // replacement: a drained leg leaves a clean slate, so no stale
            // `next_order` survives to be promoted by a later submit (the X4
            // reduce-only/flatten path mints fresh identities of its own). The next
            // quote cycle re-mints `next_order` before use, so this never strands a
            // live cycle.
            binding.active_order = None;
            binding.next_order = None;
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
        let yes = make_leg_identity("001", "eth-hourly", 1_700_000_000_000, Leg::Yes, 3);
        let no = make_leg_identity("001", "eth-hourly", 1_700_000_000_000, Leg::No, 3);
        assert_eq!(
            yes.client_order_id().as_str(),
            "001-eth-hourly-1700000000000-yes-3"
        );
        assert_eq!(
            no.client_order_id().as_str(),
            "001-eth-hourly-1700000000000-no-3"
        );
        assert_ne!(yes.client_order_id(), no.client_order_id());
        assert_eq!(yes.generation(), 3);
        assert_eq!(
            make_leg_identity("001", "eth-hourly", 1_700_000_000_000, Leg::Yes, 3),
            yes,
            "minting is a pure function of its inputs"
        );
        // Window-start injectivity: the id embeds `window_start_milliseconds`, so the
        // same (tag, market_key, leg, generation) at a different window start mints a
        // distinct id (even if the venue `market_id` were unchanged). Cross-roll and
        // cross-refill uniqueness is actually guaranteed by the monotonic
        // per-(market_key, leg) generation high-water (see `MakerRuntime::generations`),
        // not by the window start moving; this asserts the orthogonal property that the
        // window start is a real, distinguishing component of the encoding.
        let rolled = make_leg_identity("001", "eth-hourly", 1_700_003_600_000, Leg::Yes, 3);
        assert_ne!(
            yes.client_order_id(),
            rolled.client_order_id(),
            "a different window start must mint a distinct client order id"
        );
    }

    #[test]
    fn make_leg_identity_is_injective_when_market_key_contains_the_delimiter() {
        // External reviewers (PR #853) flagged a delimiter-injection collision on the
        // earlier `{tag}-{market_key}-{market_id}-{leg}-{generation}` template, where
        // the venue `market_id` could itself contain `-`. HEAD keys the id on
        // `window_start_milliseconds` (decimal) instead, and `order_id_tag` is
        // validated delimiter-free at load, so `market_key` is the only `-`-bearing
        // component and is bounded by a pure-decimal window start. The source tuple
        // therefore stays recoverable and the encoding injective. Differential: two
        // distinct markets whose key/window boundaries could be confused still mint
        // distinct ids; re-introducing a `-`-bearing component after `market_key`
        // (such as the old `market_id`) would let them collide.
        let a = make_leg_identity("001", "eth-hourly", 1_700_000_000_000, Leg::Yes, 1);
        let b = make_leg_identity("001", "eth", 1_700_000_000_000, Leg::Yes, 1);
        assert_ne!(
            a.client_order_id(),
            b.client_order_id(),
            "distinct market keys must mint distinct ids even when one contains the delimiter"
        );
        // A market_key whose trailing characters mimic another market's window prefix
        // still cannot alias, because the window start is a fixed decimal field:
        // "001-eth-1700000000000-1-yes-1" vs "001-eth-1700000000001-yes-1".
        let c = make_leg_identity("001", "eth-1700000000000", 1, Leg::Yes, 1);
        let d = make_leg_identity("001", "eth", 1_700_000_000_001, Leg::Yes, 1);
        assert_ne!(
            c.client_order_id(),
            d.client_order_id(),
            "a hyphen in market_key cannot fabricate another market's (key, window) pair"
        );
    }

    #[test]
    fn submit_dispatch_promotes_next_identity_to_active() {
        // Differential rotation guard: a Submit promotes next_order -> active_order
        // (the order now rests, so a later requote cancels it via active_order). A
        // no-op rotation would leave active_order None, so the assertion fails.
        let mut binding = leg_binding("YES.SIM");
        let minted = make_leg_identity("001", "m", 1, Leg::Yes, 1);
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
        binding.active_order = Some(make_leg_identity("001", "m", 1, Leg::No, 2));
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
    fn cancel_all_dispatch_clears_both_active_and_pending_identities() {
        // A drain clears the resting identity AND drops the pre-minted replacement,
        // so a cancelled leg leaves a clean slate (no stale next_order for a later
        // submit to promote). Differential: next_order is seeded here, so if the
        // cancel branch stops clearing it the pre-minted replacement survives the
        // drain and the second assertion fails.
        let mut binding = leg_binding("YES.SIM");
        binding.active_order = Some(make_leg_identity("001", "m", 1, Leg::Yes, 5));
        binding.next_order = Some(make_leg_identity("001", "m", 1, Leg::Yes, 6));
        rotate_leg_identity(
            &mut binding,
            Some(&MakerOrderDispatchOutcome::CanceledAll {
                leg: Some(Leg::Yes),
                instrument_id: InstrumentId::from("YES.SIM"),
                order_side: Some(OrderSide::Buy),
            }),
        );
        assert_eq!(binding.active_order, None);
        assert_eq!(
            binding.next_order, None,
            "a drain must drop the pre-minted replacement identity"
        );
    }

    #[test]
    fn modify_dispatch_keeps_active_identity() {
        // A modify amends in place: the resting identity must survive, otherwise a
        // later cancel could not target it.
        let mut binding = leg_binding("YES.SIM");
        let resting = make_leg_identity("001", "m", 1, Leg::Yes, 4);
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
        let next = make_leg_identity("001", "m", 1, Leg::No, 1);
        binding.next_order = Some(next.clone());
        rotate_leg_identity(&mut binding, None);
        assert_eq!(binding.next_order, Some(next));
        assert_eq!(binding.active_order, None);
    }
}
