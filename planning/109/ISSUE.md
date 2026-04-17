# Issue #109: resolution-basis selector generalization

## Problem

The shared reference pipeline is becoming generic, but the market-discovery and selector path still carries asset-specific assumptions.

Today the main blocker is the resolution-basis layer:

- `src/platform/resolution_basis.rs` only recognizes BTC-specific forms such as `chainlink_btcusd` and `binance_btcusdt_1m`
- `src/platform/polymarket_catalog.rs` depends on that parser to classify candidate markets
- `src/platform/ruleset.rs` currently matches market basis to ruleset basis by exact string equality

This means new families like ETH Chainlink markets can fail selection unless code is edited to add more literals.

## Scope

Make the resolution-basis and selector path generic so the platform does not need asset-specific code changes for each supported market family.

This slice should include:

- replace BTC-specific parsing with generic resolution-basis parsing
- model resolution basis as structured data or an equivalent canonical normalized form
- parse Polymarket-declared market metadata into that generic representation
- parse ruleset config into the same representation
- compare ruleset basis to market basis structurally/canonically, not by brittle ad hoc literals
- audit validation/runtime for hidden asset or venue hardcodes in this path
- preserve existing venue-family validation semantics

## Why This Matters

Without this, bolt-v2 keeps reintroducing hardcoded assumptions about BTC, ETH, Binance, or Chainlink into the platform layer.

The platform should be able to support a new eligible family because metadata and config line up, not because a new string literal was added in code.

## Non-goals

- strategy logic
- direct Chainlink plumbing inside strategy code
- multiple simultaneous active rulesets
- broad platform redesign outside the resolution-basis / selector seam

## Acceptance Criteria

- ETH Chainlink markets can be discovered and selected without adding ETH-specific literals to selector logic
- existing BTC behavior remains intact
- the selector no longer depends on fragile exact raw-string conventions for basis matching
- adding another supported family in the same venue/oracle pattern does not require new hardcoded asset strings in this path
- tests cover at least BTC and ETH basis parsing/matching, plus mismatch rejection

## Related

- follow-on to #40 ruleset/selector work
- should align with #37 shared Chainlink ingest path
- unblocks the first real ETH strategy slice
