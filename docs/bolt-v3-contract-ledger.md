# bolt-v3 Contract Ledger

Status: working normalization ledger for architecture review

This file exists to keep the bolt-v3 spec set mechanically closed.
Each load-bearing rule has one canonical owner and zero duplicate owners.

## 1. Canonical validation outcome

- invariant:
  - `just check` has one implementation with structural failure, live fatal failure, and live operational warning as the only result classes
  - unresolved current `active_or_next` target is a loud live operational warning, not a fatal validation failure
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Sections 1–2
- dependent references:
  - `docs/bolt-v3-design.md` Section 17
  - `docs/bolt-v3-schema.md` Section 8

## 2. Rotating-series load freshness

- invariant:
  - first-live Polymarket `updown` resolution uses a dynamic NautilusTrader `MarketSlugFilter` closure plus NautilusTrader `request_instruments`
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
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 3
- dependent references:
  - `docs/bolt-v3-design.md` Sections 6 and 7
  - `docs/bolt-v3-schema.md` Section 5 `[venues.<identifier>.secrets]`

## 6. Order boundary

- invariant:
  - strategies use NautilusTrader-native order types directly
  - bolt does not own an executable-order schema or intent translation layer
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 8
- dependent references:
  - `docs/bolt-v3-design.md` Section 15
  - `docs/bolt-v3-schema.md` Section 7 `[parameters.entry_order]` / `[parameters.exit_order]`

## 7. Resolved target shape

- invariant:
  - resolved targets have one universal contract plus target-kind-specific fields
  - first-live `updown` uses `condition_identifier` as `resolved_market_identifier`
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 6
- dependent references:
  - `docs/bolt-v3-runtime-contracts.md` Section 9.5
  - `docs/bolt-v3-schema.md` Section 7 `[target]`

## 8. Reference-data contract

- invariant:
  - required reference data is explicit in strategy TOML and root keyed venues
  - validation `resolvable` means the instrument exists in NT cache after venue/instrument loading, not that a live tick has already arrived
  - first-live `updown` anchor extraction is the only permitted Gamma supplement and reads `eventMetadata.priceToBeat`
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 7
- dependent references:
  - `docs/bolt-v3-schema.md` Sections 7 and 8
  - `docs/bolt-v3-design.md` Section 14

## 9. Decision-event contract

- invariant:
  - one fixed first-live event set
  - every common field and event-specific enum has explicit semantics
  - unresolved selector events may carry nullable `resolved_target_identifier`
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 9
- dependent references:
  - `docs/bolt-v3-design.md` Section 18

## 9a. First-Live Sizing And Exit Mechanics

- invariant:
  - first-live notional caps are gross USDC entry-cost terms before fees
  - fee rate must be available before entry evaluation
  - exposure and exit quantities come from NautilusTrader state, not strategy memory
  - first-live exits use only valid NautilusTrader-native sell-order construction; no blind exits and no bolt exit engine
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Sections 7.3, 7.4, and 8.4
- dependent references:
  - `docs/bolt-v3-schema.md` Section 7 `[parameters]`

## 10. Local evidence persistence

- invariant:
  - structured decision events persist through NautilusTrader custom-data registration plus NT streaming/catalog persistence
  - bolt does not add a second writer loop
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Sections 9.6 and 10
- dependent references:
  - `docs/bolt-v3-schema.md` Section 5 `[persistence]` and `[persistence.streaming]`

## 11. Release identity and deploy trust

- invariant:
  - deploy automation verifies artifacts before startup
  - deploy automation writes the release manifest
  - bolt reads release identity from that manifest and does not infer it from paths
- canonical owner:
  - `docs/bolt-v3-runtime-contracts.md` Section 11
- dependent references:
  - `docs/bolt-v3-design.md` Sections 4 and 19

## 12. NautilusTrader pin governance

- invariant:
  - the pinned NautilusTrader revision is part of the contract
  - pointer updates are a dedicated follow-up slice with re-verification
- canonical owner:
  - `docs/bolt-v3-design.md` Section 13
- dependent references:
  - `docs/bolt-v3-runtime-contracts.md` Section 9.3
  - `docs/bolt-v3-design.md` Sections 4 and 21
