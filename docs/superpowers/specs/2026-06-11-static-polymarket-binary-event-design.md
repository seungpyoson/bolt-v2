# Static Polymarket Binary Event Design

## Objective

Implement config-driven static Polymarket binary event selection without reintroducing an evidence/readiness gate. The original use case is World Cup market making, but the mechanism is a reusable selector for any configured Polymarket YES/NO binary event market. The slice makes static binary event markets selectable by configuration, so maker quoting can run against real static event markets instead of the rotating `updown` cadence family.

## Approved Scope

The first implementation slice adds a `static_binary_event` market family. It selects a configured Polymarket binary market from NautilusTrader `BinaryOption` instruments using TOML-owned identity fields: `market_slug`, optional `condition_id`, configured outcome labels, and configured `fair_probability_source`. It reuses the existing market-family registry, selected-market requirement contract, and Polymarket data-client filter mechanism.

This slice does not hardcode World Cup teams, tournament names, slugs, condition IDs, token IDs, or any other event identity. All market identity values come from TOML. It also does not add a readiness gate, an evidence gate, a live-submit gate, or an event-specific strategy fork.

## Architecture

`src/bolt_v3_market_families/static_binary_event.rs` owns static event target parsing, validation, target planning, selected-market extraction, and selected-market requirement construction. It exposes the same `MarketFamilyValidationBinding` functions as `updown`, so the existing strategy and market-family registry paths can dispatch without new parent-level branches.

`src/bolt_v3_providers/polymarket.rs` extends market-slug filtering to include static event target plans in addition to rotating `updown` target plans. This lets the NT Polymarket adapter load configured static binary markets into the instrument cache.

The existing `binary_oracle_edge_taker` selection path consumes `rotating_market_family`, `underlying_asset`, `cadence_seconds`, and `cadence_slug_token` from runtime config. Static events reuse that path by projecting `cadence_slug_token = market_slug` and `underlying_asset = event_key`, and by adding optional runtime fields for `static_condition_id`, `static_yes_outcome`, `static_no_outcome`, and `static_fair_probability_source`. The market-family implementation ignores cadence for static event selection and matches on the configured static market slug, optional condition ID, and configured outcome labels.

For static binary events, `fair_probability_up` is not derived from strike/spot pricing. This slice preserves `fair_probability_source = "reference_current_price"` as the only supported static-event fair-probability source token, but the source table, provider runtime, and selected current-price plumbing are owned by PR 730. This does not add a standalone event-specific maker strategy and does not fake rotating-market pricing inputs for static event markets.

The static family binds the existing `MarketFamilyValidationBinding` maker hooks by delegating binary quote targets, settlement payout, and binary fee curve behavior to `updown`. The shared maker quote/order pipeline itself is owned by PR 716.

No event-specific quote lifecycle, order-intent, order-compile, or dispatch implementation is added in this PR. Those shared maker mechanics remain outside this static-event selection slice.

## Testing

Verification is evidence-driven. Production behavior is proven by automated Rust tests on exact-head PR CI, with local non-compile checks and scope/leakage scans before push. The required behavioral evidence proves:

- `static_binary_event` is registered as a known market family.
- Static event targets select matching YES/NO Polymarket `BinaryOption` instruments by configured market slug and outcome labels.
- Static event targets require `fair_probability_source = "reference_current_price"` and project that symbolic source into runtime config without implementing the PR 730 source table.
- Static event maker hook binding delegates binary quote layout, settlement payout, and fee curve behavior to the existing market-family hooks without adding a shared maker pipeline implementation.
- Mismatched slugs, duplicate outcomes, expired instruments, and optional condition-ID mismatches fail closed.
- Polymarket data-client mapping includes static event market slugs in its market-slug filters.

## Follow-On Scope

After static event selection lands, PR 730 must provide the referenced current-price fair-probability source and PR 716 must provide the shared maker quote/order pipeline. A later World Cup runtime shell can use those pieces, but it must not create a second standalone World Cup maker strategy, duplicate maker math, or a parallel NT order-construction path.
