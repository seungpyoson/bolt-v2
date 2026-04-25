# bolt-v3 Contract Ledger

Status: working normalization ledger for architecture review

This file exists to keep the bolt-v3 spec set mechanically closed.
Each load-bearing rule has one canonical owner and zero duplicate owners.

## 1. Canonical validation outcome

- invariant:
  - `just check` has one implementation with structural failure, live fatal failure, and live operational warning as the only result classes
  - no currently selectable `active_or_next` target is a loud live operational warning, not a fatal validation failure
  - current `updown` readiness gates run in live validation before order readiness is reported
  - missing or invalid current `updown` event-page mapping or price-to-beat evidence is a fatal order-readiness failure
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Sections 1–2
- dependent references:
  - `docs/bolt-v3-design.md` Section 17
  - `docs/bolt-v3-schema.md` Section 8
- implementation status:
  - contract accepted; implementation evidence required

## 2. Rotating-series load freshness

- invariant:
  - current Polymarket `updown` market selection uses a dynamic NautilusTrader `MarketSlugFilter` closure plus NautilusTrader `request_instruments`
  - instrument requests happen at startup and when the current/next slug pair changes, not on every retry tick after a successful request for that slug pair
  - bolt does not own a second discovery loop
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 5.3
- dependent references:
  - `docs/bolt-v3-design.md` Section 12
  - `docs/bolt-v3-schema.md` Section 5 `[venues.<identifier>.data]`

## 3. Root risk authority

- invariant:
  - `risk.default_max_notional_per_order` is the hard entity cap
  - strategy `order_notional_target` is the desired local sizing target
  - synchronization to NT per-instrument caps follows NT instrument topics, not strategy callbacks
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 4
- dependent references:
  - `docs/bolt-v3-schema.md` Sections 5 and 7
  - `docs/bolt-v3-design.md` Sections 6 and 16

## 4. Identity model

- invariant:
  - root `trader_identifier` is the canonical process/trader identity
  - Nautilus `StrategyId` is derived as `"{strategy_archetype}-{order_id_tag}"`
  - `strategy_instance_identifier` is operator-facing and forensic-facing, not a second NT strategy identity
- canonical owner:
  - `docs/bolt-v3-schema.md` Sections 5 and 7
- dependent references:
  - `docs/bolt-v3-design.md` Section 9
  - `docs/bolt-v3-runtime-contracts.md` Section 9.3

## 5. Secret source and env fallback

- invariant:
  - SSM is the only secret source
  - env-var credential fallbacks are forbidden for every keyed venue with `[secrets]`
  - forbidden variable lists are derived from venue kind, environment, and product type before NautilusTrader client construction
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 3
- dependent references:
  - `docs/bolt-v3-design.md` Sections 6 and 7
  - `docs/bolt-v3-schema.md` Section 5 `[venues.<identifier>.secrets]`
- implementation status:
  - contract accepted; env-blocking tests required

## 6. Order boundary

- invariant:
  - strategies use NautilusTrader-native order types directly
  - bolt does not own an executable-order schema or intent translation layer
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 8
- dependent references:
  - `docs/bolt-v3-design.md` Section 15
  - `docs/bolt-v3-schema.md` Section 7 `[parameters.entry_order]` / `[parameters.exit_order]`

## 7. Target-stack data model

- invariant:
  - the target stack has five separate shapes: `configured_updown_target`, `selected_market`, `updown_selected_market_facts`, `market_selection_result`, and `updown_market_mechanical_result`
  - `configured_updown_target` is configuration only and comes from strategy TOML plus the configured venue reference
  - `selected_market` is identity and market-boundary data only
  - `updown_selected_market_facts` owns the selected-market observation timestamp and price-to-beat cluster
  - `market_selection_result` uses `current` and `next` as roles, not object types
  - successful `market_selection_result` contains `configured_updown_target`, `market_selection_timestamp_milliseconds`, and `updown_selected_market_facts`; it does not duplicate `selected_market` as a sibling field
  - `updown_market_mechanical_result` is family mechanical evaluation before edge, side, sizing, order construction, or pre-submit rejection, with `updown_market_mechanical_outcome` as its verdict field
  - current `updown` live-trading scope uses `polymarket_condition_id` as the primary selected-market identifier
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 6
- dependent references:
  - `docs/bolt-v3-runtime-contracts.md` Section 9.5
  - `docs/bolt-v3-schema.md` Section 7 `[target]`

## 8. Reference-data contract

- invariant:
  - required reference data is explicit in strategy TOML and root keyed venues
  - validation `resolvable` means the instrument exists in NT cache after venue/instrument loading, not that a live tick has already arrived
  - current `updown` launch scope uses the Gamma event page as the only source for `price_to_beat_value`; it is a narrow supplement to NautilusTrader-loaded market state, not a discovery system
  - each implementation-proven Gamma event page returns exactly one Gamma event with numeric positive `eventMetadata.priceToBeat`
  - once the mapping rule is accepted, each Gamma event links back to the selected NautilusTrader / Polymarket CLOB market by that runtime-owned mapping contract
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 7
- dependent references:
  - `docs/bolt-v3-schema.md` Sections 7 and 8
  - `docs/bolt-v3-design.md` Section 14
  - `docs/bolt-v3-runtime-contracts.md` Sections 1–2
- implementation status:
  - contract accepted; event page slug mapping and live Gamma readiness evidence required

### 8a. S1 event page slug mapping evidence task

- invariant:
  - the runtime contract reserves ownership of how a selected NautilusTrader / Polymarket CLOB market maps to the Gamma event page used for `price_to_beat_value`
  - the mapping rule is currently unset and blocks current `updown` order readiness
  - S1 must produce evidence which can be promoted into the runtime-owned mapping rule
  - S5 validates only the implementation-proven Gamma event page and JSON path from runtime-owned mapping evidence, not alternate price-to-beat sources
- required S1 checks, in order:
  - NautilusTrader loaded `BinaryOption.info` fields
  - raw Gamma `/markets?slug=<clob_market_slug>` response
  - raw Gamma `/events?slug=<candidate_slug>` response
  - fields linking market slug, event slug, condition identifier, Gamma market identifier, token identifiers, or question identifier
- required S1 result:
  - `MAPPING_FOUND`: exact field/path and example payload
  - `MAPPING_DERIVED`: deterministic derivation rule and proof on current and next configured `updown` targets
  - `MAPPING_NOT_FOUND`: checked-path proof; S6 chooses whether to defer `updown` or patch/upstream
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Sections 2, 5.3, 6.3, and 6.4
- dependent references:
  - S1 Market Identity implementation evidence
- implementation status:
  - blocker for current `updown` order readiness until the mapping rule is written into `docs/bolt-v3-runtime-contracts.md` Section 6.4

## 9. Decision-event contract

- invariant:
  - one fixed current event set
  - every common field and event-specific enum has explicit semantics
  - common event fields use `venue_config_key`, not a second venue-key synonym
  - `market_selection_result` events use `market_selection_outcome` as the flattened outcome field
  - `configured_target_id` is present on every decision event
  - order-submission events require a non-null `client_order_id`
  - pre-submit rejection events carry their site-scoped rejection reason
  - every fixed event type has a Rust custom-data schema with explicit nullable fields
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 9
- dependent references:
  - `docs/bolt-v3-design.md` Section 18
- implementation status:
  - contract accepted; schema and round-trip tests required

## 9a. Current Sizing And Exit Mechanics

- invariant:
  - current notional caps are gross USDC entry-cost terms before fees
  - fee rate must be available before entry evaluation
  - exposure and exit quantities come from NautilusTrader state, not strategy memory
  - current exits use only valid NautilusTrader-native sell-order construction; no blind exits and no bolt exit engine
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Sections 7.3, 7.4, and 8.4
- dependent references:
  - `docs/bolt-v3-schema.md` Section 7 `[parameters]`
  - `docs/bolt-v3-runtime-contracts.md` Section 9.5 `entry_evaluation`
  - `docs/bolt-v3-runtime-contracts.md` Section 9.5 `entry_pre_submit_rejection`
  - `docs/bolt-v3-runtime-contracts.md` Section 9.5 `exit_evaluation`
  - `docs/bolt-v3-runtime-contracts.md` Section 9.5 `exit_order_submission`
  - `docs/bolt-v3-runtime-contracts.md` Section 9.5 `exit_pre_submit_rejection`
- implementation status:
  - contract accepted; S2/S3/S4 production-path coverage required

## 10. Local evidence persistence

- invariant:
  - structured decision events persist through NautilusTrader custom-data registration plus one canonical catalog write path
  - every decision event is constructed as a registered NautilusTrader custom-data value before emission
  - submit-gating events require accepted handoff to the single canonical in-process persistence path before NautilusTrader order submit
  - accepted handoff does not require durable catalog flush completion
  - construction or handoff failure blocks the current submit for submit-gating events
  - durable catalog persistence failure blocks future order submission for the emitting strategy
  - bolt does not add a second writer loop, subscriber-writer loop, or parallel persistence path
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Sections 9.6, 9.7, and 10
- dependent references:
  - `docs/bolt-v3-schema.md` Section 5 `[persistence]` and `[persistence.streaming]`
- implementation status:
  - contract accepted; catalog round-trip and failure tests required

## 11. Release identity and deploy trust

- invariant:
  - deploy automation verifies artifacts before startup
  - deploy automation writes the release manifest
  - bolt reads release identity from that manifest and does not infer it from paths
  - release manifest records artifact hashes, config hash, build profile, and NautilusTrader revision
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 11
- dependent references:
  - `docs/bolt-v3-design.md` Sections 4 and 19
- implementation status:
  - contract accepted; deploy evidence required

## 12. NautilusTrader pin governance

- invariant:
  - the pinned NautilusTrader revision is part of the contract
  - Cargo dependency metadata and lock/build metadata are the dependency source of truth
  - Section 9.3 `nautilus_trader_revision`, release manifest `nautilus_trader_revision`, emitted revision, and compiled pin must agree
  - pointer updates are a dedicated verification slice with CLOB V2 readiness, panic gate, and ledger re-verification
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 11.5
- dependent references:
  - `docs/bolt-v3-runtime-contracts.md` Section 9.3
  - `docs/bolt-v3-runtime-contracts.md` Section 11.2
  - `docs/bolt-v3-runtime-contracts.md` Section 12
  - `docs/bolt-v3-runtime-contracts.md` Section 13
- implementation status:
  - contract accepted; re-verification required for future pin changes

## 13. Panic and service policy

- invariant:
  - the current panic gate runs against the exact pinned release artifact and exact systemd restart policy
  - crash dumps are disabled
  - restart loops are capped with numeric settings
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Sections 11.4 and 12
- dependent references:
  - `docs/bolt-v3-design.md` Sections 19 and 20
- implementation status:
  - contract accepted; issue `#239` evidence required

## 14. CLOB V2 readiness

- invariant:
  - live capital is blocked unless the pinned NautilusTrader Polymarket adapter is verified against the current Polymarket CLOB signing requirement
  - the verified CLOB signing version is recorded in release evidence
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 13
- dependent references:
  - `docs/bolt-v3-design.md` Section 13
- implementation status:
  - blocker; verification evidence required
