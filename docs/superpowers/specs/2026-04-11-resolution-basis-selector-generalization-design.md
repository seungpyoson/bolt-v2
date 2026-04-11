# Resolution-Basis Selector Generalization

## Status

Draft design for review.

## Scope

This spec covers issue `#109`: generalize the resolution-basis and selector seam so market discovery and selection do not depend on asset-specific literals.

This slice includes:

- a typed internal `ResolutionBasis` model
- generic parsing of ruleset config strings into that model
- generic parsing of Polymarket market metadata into that model
- structural matching in selector eligibility
- validation derived from the typed basis instead of string-prefix heuristics
- generic regression coverage across multiple market fixtures
- follow-on tracking for a future structured config schema

This spec does **not** include:

- strategy logic changes
- multiple simultaneous active rulesets
- a broad platform redesign outside the resolution-basis / selector seam
- changing the operator-facing TOML schema for `resolution_basis` in this issue

## Problem

The current selector seam looks generic at a glance, but it is still asset-shaped in code:

- [src/platform/resolution_basis.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/resolution_basis.rs:1) only recognizes BTC-specific forms such as `chainlink_btcusd` and `binance_btcusdt_1m`
- [src/platform/polymarket_catalog.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/polymarket_catalog.rs:82) translates Polymarket `GammaMarket` rows into `CandidateMarket` using that parser
- [src/platform/ruleset.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/ruleset.rs:122) compares market basis to ruleset basis by exact raw-string equality
- [src/validate.rs](/Users/spson/Projects/Claude/bolt-v2/src/validate.rs:442) only enforces the venue-family prefix, not the full structured basis

That mismatch produces a brittle system:

- validation can appear generic enough to allow new eligible families
- selection still depends on literal strings and formatting conventions
- adding a new supported family member should be data-only, but today it can require code edits

## Root Cause

The root cause is not that Polymarket has two metadata fields. The root cause is that `bolt-v2` does not normalize the available metadata into one typed model before matching.

Gamma market payloads expose both:

- `resolution_source`: typically the cleanest source pointer, often enough to identify venue/provider and pair
- `description`: rule text that can contain additional semantics such as candle interval or settlement details

Today the parser already accepts both inputs, but the catalog translation path only passes `description` and ignores `resolution_source`. Even worse, the output is a BTC-shaped string instead of structured basis data.

## Design Principles

1. No asset-specific selector branches.
2. No venue-specific selector branches beyond explicit basis-family parsing.
3. Fail closed on ambiguous or conflicting basis metadata.
4. Keep the current TOML boundary stable for `#109`.
5. Make the internal model the future source of truth so structured TOML can be added later without rewriting selector/runtime logic.

## Internal Model

Introduce a typed internal `ResolutionBasis` in [src/platform/resolution_basis.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/resolution_basis.rs:1).

Recommended shape:

```rust
enum ResolutionBasis {
    ExchangeCandle {
        venue: ReferenceVenueKind,
        base: String,
        quote: String,
        interval: CandleInterval,
    },
    OraclePriceFeed {
        provider: ReferenceVenueKind,
        base: String,
        quote: String,
    },
}
```

Recommended interval model:

```rust
enum CandleInterval {
    OneMinute,
    FiveMinutes,
    FifteenMinutes,
    OneHour,
}
```

This issue only needs the basis families actually present in the current platform seam:

- exchange-candle resolution
- oracle-price-feed resolution

That is intentionally narrower than a speculative universal descriptor. The model must be strong enough to remove hardcoded assets now and stable enough to back a later structured TOML schema.

## Config Boundary

For issue `#109`, keep the config boundary unchanged:

```toml
resolution_basis = "binance_btcusdt_1m"
```

Add a generic parser:

```rust
parse_ruleset_resolution_basis(&str) -> Result<ResolutionBasis, ResolutionBasisParseError>
```

The parser should accept the current flat-string grammar generically:

- exchange candle: `<venue>_<base><quote>_<interval>`
- oracle feed: `<provider>_<base><quote>`

The parser must not hardcode specific assets. It should normalize venue/provider, base, quote, and interval from the string grammar and reject malformed or unsupported shapes explicitly.

## Market Metadata Parsing

Replace the current BTC-shaped market parser with a typed parser that consumes both metadata fields:

```rust
parse_declared_resolution_basis(
    resolution_source: Option<&str>,
    description: Option<&str>,
) -> Option<ResolutionBasis>
```

In [src/platform/polymarket_catalog.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/polymarket_catalog.rs:82), pass:

- `market.resolution_source.as_deref()`
- `market.description.as_deref()`

Parsing policy:

- use `resolution_source` first for source identity such as venue/provider and pair
- use `description` to fill in missing semantics such as interval/window
- if both sources provide the same field and they disagree, reject the market as unparseable for selection
- if required basis fields remain missing after merge, reject the market as unparseable for selection

This is a fail-closed policy. The selector should never guess through conflicting resolution metadata.

## Selector Matching

Change [src/platform/ruleset.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/ruleset.rs:5) so `CandidateMarket.declared_resolution_basis` stores the typed `ResolutionBasis`, not a raw string.

Keep `RulesetConfig.resolution_basis` as `String` in [src/config.rs](/Users/spson/Projects/Claude/bolt-v2/src/config.rs:150) and [src/live_config.rs](/Users/spson/Projects/Claude/bolt-v2/src/live_config.rs:356) for this issue.

Selector policy:

- parse the ruleset basis once at the start of evaluation
- compare ruleset basis and market basis structurally
- reject on any difference in basis family, venue/provider, base, quote, or interval where applicable

The selector must stop asking whether two raw strings are spelled the same and instead ask whether the ruleset and the market describe the same resolution source.

## Validation

Keep `resolution_basis` as a required non-empty string in config for this issue, but add typed validation:

- invalid `resolution_basis` grammar must fail validation explicitly
- reference-venue-family validation must derive from parsed `ResolutionBasis`, not from raw string prefixes

Replace [implied_reference_venue_kind(...)](/Users/spson/Projects/Claude/bolt-v2/src/validate.rs:442) with a typed helper that answers:

```rust
required_reference_venue_kind(&ResolutionBasis) -> Option<ReferenceVenueKind>
```

Validation semantics stay the same:

- an exchange-candle basis requires a matching configured reference venue family
- an oracle-price-feed basis requires the matching configured reference venue family

This preserves current runtime safety while removing fragile prefix inference.

## Tests

Use generic, table-driven fixtures instead of asset-specific logic.

### Catalog And Parsing Tests

Expand [tests/polymarket_catalog.rs](/Users/spson/Projects/Claude/bolt-v2/tests/polymarket_catalog.rs) to cover:

- exchange-candle basis fixtures with varying venue / base / quote / interval
- oracle-price-feed fixtures with varying provider / base / quote
- cases where `resolution_source` is sufficient by itself
- cases where `description` adds missing semantics such as interval
- conflicting `resolution_source` / `description` cases that must fail closed
- malformed and incomplete metadata that must be rejected

### Selector Tests

Expand [tests/ruleset_selector.rs](/Users/spson/Projects/Claude/bolt-v2/tests/ruleset_selector.rs) so selector eligibility is proven against typed equality:

- equivalent structured basis matches even when raw formatting differs
- true structured mismatches reject correctly
- no selector branch depends on any specific asset literal

### Validation Tests

Expand [src/validate/tests.rs](/Users/spson/Projects/Claude/bolt-v2/src/validate/tests.rs) to cover:

- invalid `resolution_basis` grammar
- required reference venue family derived from parsed basis
- continued acceptance of valid current flat-string config values

## File-Level Change Plan

- [src/platform/resolution_basis.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/resolution_basis.rs:1)
  Introduce `ResolutionBasis`, parsing, merge logic, and typed helpers.
- [src/platform/polymarket_catalog.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/polymarket_catalog.rs:82)
  Parse from both `resolution_source` and `description`; store typed basis on `CandidateMarket`.
- [src/platform/ruleset.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/ruleset.rs:5)
  Compare typed basis structurally instead of raw strings.
- [src/validate.rs](/Users/spson/Projects/Claude/bolt-v2/src/validate.rs:442)
  Replace string-prefix inference with typed basis validation.
- [tests/polymarket_catalog.rs](/Users/spson/Projects/Claude/bolt-v2/tests/polymarket_catalog.rs:1)
  Add generic parsing fixtures and conflict tests.
- [tests/ruleset_selector.rs](/Users/spson/Projects/Claude/bolt-v2/tests/ruleset_selector.rs:1)
  Add structural selector matching coverage.
- [src/validate/tests.rs](/Users/spson/Projects/Claude/bolt-v2/src/validate/tests.rs:2205)
  Add typed validation coverage.

## Acceptance Criteria

This design satisfies issue `#109` when:

- Polymarket market metadata is normalized into typed basis data without asset-specific selector code
- the selector no longer depends on exact raw-string equality for basis matching
- adding another supported family member inside an already-supported basis family does not require new asset literals in this path
- current valid behavior remains intact
- generic tests cover successful parsing/matching, mismatch rejection, and malformed metadata rejection

## Follow-On Tracking

This issue deliberately keeps `resolution_basis` as a flat config string.

Follow-on work should move operator config to a structured tagged representation, such as:

```toml
[rulesets.resolution_basis]
kind = "exchange_candle"
venue = "binance"
base = "BASE"
quote = "QUOTE"
interval = "1h"
```

That follow-on should reuse the internal `ResolutionBasis` introduced here as the source of truth. The future schema migration should be a config/materialization change, not another selector rewrite.

At design time, no existing GitHub issue was found in `seungpyoson/bolt-v2` for that follow-on. If none exists at implementation time, create one and link it from `#109`.
