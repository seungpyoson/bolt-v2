# Resolution-Basis Selector Generalization

## Status

Draft design for review.

## Scope

This spec covers issue `#109`: generalize the resolution-basis and selector seam so market discovery and selection do not depend on asset-specific literals.

This slice includes:

- a typed internal `ResolutionBasis` model
- generic parsing of ruleset config strings into that model
- typed parsing of currently-available Polymarket market metadata into that model
- structural matching in selector eligibility
- validation derived from the typed basis instead of string-prefix heuristics
- generic regression coverage across multiple market fixtures
- immediate follow-on tracking for adapter and config-schema work

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
- [src/validate.rs](/Users/spson/Projects/Claude/bolt-v2/src/validate.rs:442) maintains a separate string-prefix table to infer required reference venue family

That mismatch produces a brittle system:

- validation can appear generic enough to allow new eligible families
- selection still depends on literal strings and formatting conventions
- parser and validation each carry their own string-based source of truth
- adding a new supported family member should be data-only, but today it can require code edits

## Root Cause

The root cause is that `bolt-v2` does not normalize config and market metadata into one typed model before matching, and instead duplicates resolution-basis knowledge across two separate string-based seams.

Today those seams are:

- the resolution-basis parser in [src/platform/resolution_basis.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/resolution_basis.rs:1)
- the prefix table in [src/validate.rs](/Users/spson/Projects/Claude/bolt-v2/src/validate.rs:442)

There is also an upstream dependency constraint that affects scope:

- Polymarket Gamma JSON includes `resolutionSource`
- the pinned `nautilus-polymarket` `GammaMarket` struct in NT `af2aefc` does **not** expose that field, so the current runtime seam only has `description`

Issue `#109` therefore fixes the typed matching problem on the metadata we actually have in-process today, and explicitly tracks adapter surfacing of `resolutionSource` as immediate follow-on work.

## Design Principles

1. No asset-specific selector branches.
2. No venue-specific selector branches beyond explicit basis-family parsing.
3. Keep the resolution-source domain separate from the reference-venue config domain.
4. Keep the current TOML boundary stable for `#109`.
5. Make the internal model the future source of truth so structured TOML can be added later without rewriting selector/runtime logic.
6. Use an opaque normalized `pair` string in this issue; do not split `base` and `quote` yet.
7. Be explicit that description parsing remains heuristic extraction for known basis families, while selector and validation become generic downstream.

## Internal Model

Introduce a typed internal `ResolutionBasis` in [src/platform/resolution_basis.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/resolution_basis.rs:1).

Recommended shape:

```rust
enum ResolutionSourceKind {
    Binance,
    Bybit,
    Deribit,
    Hyperliquid,
    Kraken,
    Okx,
    Chainlink,
}

enum ResolutionBasis {
    ExchangeCandle {
        source: ResolutionSourceKind,
        pair: String,
        interval: CandleInterval,
    },
    OraclePriceFeed {
        source: ResolutionSourceKind,
        pair: String,
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

`ResolutionSourceKind` is intentionally separate from `ReferenceVenueKind`.

- `ResolutionSourceKind` describes what a market says it resolves against
- `ReferenceVenueKind` describes what reference venues our runtime can be configured to consume

Validation should map from `ResolutionBasis` to a required `ReferenceVenueKind` where such a mapping exists. The two domains must not share one enum just because they overlap today.

## Config Boundary

For issue `#109`, keep the config boundary unchanged:

```toml
resolution_basis = "binance_btcusdt_1m"
```

Add a generic parser:

```rust
parse_ruleset_resolution_basis(&str) -> Result<ResolutionBasis, ResolutionBasisParseError>
```

The parser should accept the current flat-string grammar using known source identifiers:

- exchange candle: `<source>_<pair>_<interval>`
- oracle feed: `<source>_<pair>`

The parser must not hardcode specific assets. It should normalize source, pair, and interval from the string grammar and reject malformed or unsupported shapes explicitly.

The parser must match against known source identifiers rather than trying to infer basis family from string shape alone.

For valid current config strings, the parser must be invertible:

```rust
parse_ruleset_resolution_basis(s)?.to_string() == s
```

That canonicalization rule is the contract that keeps config parsing and selector matching aligned until structured TOML replaces the flat string.

## Market Metadata Parsing

Replace the current BTC-shaped market parser with a typed parser over the metadata actually available in-process for `#109`:

```rust
parse_declared_resolution_basis(description: Option<&str>) -> Option<ResolutionBasis>
```

In [src/platform/polymarket_catalog.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/polymarket_catalog.rs:82), parse from:

- `market.description.as_deref()`

Parsing policy:

- parse known supported settlement families out of the current description text
- normalize the extracted data through the same canonicalization path used by config parsing
- reject malformed or incomplete descriptions that do not yield a complete supported `ResolutionBasis`

This is still a fail-closed policy, but it is honest about the current adapter seam: downstream matching becomes typed and generic, while upstream description extraction remains heuristic for supported families.

When the adapter later exposes `resolutionSource`, extend the parser policy to:

- parse `resolution_source` independently
- parse `description` independently
- accept if both parsed values agree or one side is absent
- reject if both sides parse and differ structurally

That merge behavior is follow-on work, not part of `#109`.

## Selector Matching

Change [src/platform/ruleset.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/ruleset.rs:5) so `CandidateMarket.declared_resolution_basis` stores the typed `ResolutionBasis`, not a raw string.

Keep `RulesetConfig.resolution_basis` as `String` in [src/config.rs](/Users/spson/Projects/Claude/bolt-v2/src/config.rs:150) and [src/live_config.rs](/Users/spson/Projects/Claude/bolt-v2/src/live_config.rs:356) for this issue.

Selector policy:

- parse the ruleset basis once at the start of evaluation
- compare ruleset basis and market basis structurally
- reject on any difference in basis family, source, pair, or interval where applicable

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

This preserves current runtime safety while deleting the second string-based source of truth in validation.

## Tests

Use generic, table-driven fixtures instead of asset-specific logic.

### Catalog And Parsing Tests

Expand [tests/polymarket_catalog.rs](/Users/spson/Projects/Claude/bolt-v2/tests/polymarket_catalog.rs) to cover:

- exchange-candle basis fixtures with varying source / pair / interval
- oracle-price-feed fixtures with varying source / pair
- description-only fixtures for supported settlement families
- malformed and incomplete metadata that must be rejected
- a canonicalization check that market parsing lands on the same `ResolutionBasis` shape expected by config parsing for equivalent current conventions

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
- parser invertibility for valid current flat-string config values

## File-Level Change Plan

- [src/platform/resolution_basis.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/resolution_basis.rs:1)
  Introduce `ResolutionBasis`, `ResolutionSourceKind`, canonicalization, config parsing, and typed helpers.
- [src/platform/polymarket_catalog.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/polymarket_catalog.rs:82)
  Parse from the current market description seam; store typed basis on `CandidateMarket`.
- [src/platform/ruleset.rs](/Users/spson/Projects/Claude/bolt-v2/src/platform/ruleset.rs:5)
  Compare typed basis structurally instead of raw strings.
- [src/validate.rs](/Users/spson/Projects/Claude/bolt-v2/src/validate.rs:442)
  Replace string-prefix inference with typed basis validation.
- [tests/polymarket_catalog.rs](/Users/spson/Projects/Claude/bolt-v2/tests/polymarket_catalog.rs:1)
  Add generic parsing fixtures and canonicalization tests.
- [tests/ruleset_selector.rs](/Users/spson/Projects/Claude/bolt-v2/tests/ruleset_selector.rs:1)
  Add structural selector matching coverage.
- [src/validate/tests.rs](/Users/spson/Projects/Claude/bolt-v2/src/validate/tests.rs:2205)
  Add typed validation coverage.
- [tests/platform_runtime.rs](/Users/spson/Projects/Claude/bolt-v2/tests/platform_runtime.rs:1)
  Update runtime fixtures to use typed `declared_resolution_basis`.
- [tests/test_nan_regression.rs](/Users/spson/Projects/Claude/bolt-v2/tests/test_nan_regression.rs:1)
  Update regression fixtures to use typed `declared_resolution_basis`.

## Acceptance Criteria

This design satisfies issue `#109` when:

- Polymarket market metadata is normalized into typed basis data without asset-specific selector code
- the selector no longer depends on exact raw-string equality for basis matching
- adding another supported family member inside an already-supported basis family does not require new asset literals in this path
- config parsing and selector matching share one canonical representation for valid current flat-string ruleset values
- current valid behavior remains intact
- generic tests cover successful parsing/matching, mismatch rejection, and malformed metadata rejection

## Follow-On Tracking

This issue deliberately keeps `resolution_basis` as a flat config string and is constrained by the current adapter surface, which does not expose `resolutionSource` on `GammaMarket` at runtime even though the upstream JSON includes it.

The follow-on issue is now tracked as `#114` and should be linked from `#109`.

That follow-on must cover two items together:

1. surface `resolutionSource` on the Polymarket adapter `GammaMarket` model
2. move operator config to a structured tagged representation

The future structured config should look like:

```toml
[rulesets.resolution_basis]
kind = "exchange_candle"
source = "binance"
pair = "PAIR"
interval = "1h"
```

That follow-on should reuse the internal `ResolutionBasis` introduced here as the source of truth. The future schema migration should be a config/materialization change, and the future adapter enhancement should add parsed-value merge and conflict detection between `resolutionSource` and `description`, not another selector rewrite.

Follow-on tracking issue: `#114` "expose Gamma resolutionSource and migrate resolution_basis to structured config".
