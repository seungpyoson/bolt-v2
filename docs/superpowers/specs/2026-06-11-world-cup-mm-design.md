# World Cup MM Static Binary Event Design

## Objective

Implement the first production-aligned World Cup market-making slice without reintroducing an evidence/readiness gate. The slice makes Polymarket World Cup-style binary event markets selectable by configuration, so maker quoting can run against real static event markets instead of the rotating `updown` cadence family.

## Approved Scope

The first implementation slice adds a `static_binary_event` market family. It selects a configured Polymarket binary market from NautilusTrader `BinaryOption` instruments using TOML-owned identity fields: `market_slug`, optional `condition_id`, configured outcome labels, and configured `fair_probability_source`. It reuses the existing market-family registry, selected-market requirement contract, and Polymarket data-client filter mechanism.

This slice does not hardcode World Cup teams, tournament names, slugs, condition IDs, or token IDs. All market identity values come from TOML. It also does not add a readiness gate, an evidence gate, a live-submit gate, or a World Cup-specific strategy fork.

## Architecture

`src/bolt_v3_market_families/static_binary_event.rs` owns static event target parsing, validation, target planning, selected-market extraction, and selected-market requirement construction. It exposes the same `MarketFamilyValidationBinding` functions as `updown`, so the existing strategy and market-family registry paths can dispatch without new parent-level branches.

`src/bolt_v3_providers/polymarket.rs` extends market-slug filtering to include static event target plans in addition to rotating `updown` target plans. This lets the NT Polymarket adapter load configured World Cup markets into the instrument cache.

The existing `binary_oracle_edge_taker` selection path consumes `rotating_market_family`, `underlying_asset`, `cadence_seconds`, and `cadence_slug_token` from runtime config. Static events reuse that path by projecting `cadence_slug_token = market_slug` and `underlying_asset = event_key`, and by adding optional runtime fields for `static_condition_id`, `static_yes_outcome`, `static_no_outcome`, and `static_fair_probability_source`. The market-family implementation ignores cadence for static event selection and matches on the configured static market slug, optional condition ID, and configured outcome labels.

For static sports events, `fair_probability_up` is not derived from strike/spot pricing. Static targets require `fair_probability_source = "reference_current_price"` plus a strategy-owned `[reference_current_price]` source table, which lets the existing PR #606 reference-current-price path provide the configured fair probability. This does not add a standalone World Cup maker strategy and does not fake rotating-market pricing inputs for static event markets.

`src/bolt_v3_maker_quote_plan.rs` composes the existing maker primitives for any market family: reference fair probability, optional book microprice nudge, Glosten-Milgrom reservation bid/ask, inventory skew, and the family-owned quote layout. Static World Cup markets use this through the same `MarketFamilyValidationBinding` quote-target seam as `updown`; no duplicate World Cup maker math exists.

`src/bolt_v3_maker_quote_control.rs` composes the existing quote lifecycle with the existing requote budget so a denied venue budget does not advance lifecycle state. `src/bolt_v3_maker_quote_set.rs` applies family-produced YES/NO quote targets to that control layer and gates new submit actions through the existing per-market reservation helper across both legs before committing lifecycle or budget state. `src/bolt_v3_maker_order_plan.rs` binds approved lifecycle actions to caller-supplied instruments and maker order identities, producing explicit submit/cancel/modify intents without fabricating client order IDs. `src/bolt_v3_maker_order_compile.rs` compiles those approved maker order intents into the existing `NtOrderTemplate` and `NtOrderBuildInputs` path, preserving one NT order-construction seam instead of adding a World Cup-specific submit path. `src/bolt_v3_maker_order_dispatch.rs` dispatches compiled maker commands through that same NT order builder and a caller-owned runtime sink for submit/cancel/modify, keeping command mechanics shared rather than embedded in a strategy fork.

## Testing

Implementation uses TDD. The first failing tests prove:

- `static_binary_event` is registered as a known market family.
- Static event targets select matching YES/NO Polymarket `BinaryOption` instruments by configured market slug and outcome labels.
- Static event targets require `fair_probability_source = "reference_current_price"` and a `[reference_current_price]` source table, then project the fair-probability source into runtime config.
- Static event maker quote planning uses the shared maker model and static market-family quote layout.
- Mismatched slugs, duplicate outcomes, expired instruments, and optional condition-ID mismatches fail closed.
- Polymarket data-client mapping includes static event market slugs in its market-slug filters.
- Planned YES/NO maker quote targets are budget-gated and reservation-gated before lifecycle state mutates.
- Approved quote lifecycle actions materialize explicit maker order intents only when the required active or next order identity is present.
- Maker submit intents compile into post-only GTC limit order build inputs through the existing NT order template path, while cancel/modify intents preserve the active maker client-order identity.
- Compiled maker commands dispatch through the existing NT order builder before submit, preserve active identities for cancel/modify, and fail before calling the runtime sink when order construction fails.

## Follow-On Scope

After static event selection, reference-current-price fair-probability sourcing, shared maker quote planning, quote lifecycle control, reservation gating, order-intent planning, NT order-input compilation, and shared command dispatch exist, the next slice must connect the runtime strategy shell's state/config inputs to the maker quote pipeline and implement the sink against the strategy's NT submit/cancel/modify methods. It must not create a second standalone World Cup maker strategy, duplicate maker math, or add a parallel NT order-construction path.
