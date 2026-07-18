# Review Closure Boundaries Design

## Context

PR #1447 updates Bolt to NautilusTrader revision
`d81be0bcc7a473c45d2dc8a8885638336073a218`. Successive adversarial reviews
found two recurring design weaknesses in the remediation work:

1. a fill-void safety property was attached to a builder's declared strategy
   type while overridable builder methods could construct or register another
   type; and
2. data-client schema authority was duplicated in Bolt-maintained field
   allowlists even though the pinned NautilusTrader config types already reject
   unknown fields.

The same review also identified missing fail-closed regression evidence for
newly required Hyperliquid and Polymarket controls and Kraken's validated
Spot/Demo invariant.

## Goals

- Make the strategy type constrained by `FillVoidPolicyGuard` the only type a
  registered builder can construct and pass to `Trader::add_strategy`.
- Retain exactly one data-client schema authority: the pinned NautilusTrader
  config type, with Bolt adding only its product-specific credential and
  required-control policy.
- Make validation and adapter mapping use the same parsing boundary.
- Add fail-closed evidence for every newly required provider control and the
  Kraken invariant.
- Avoid provider ladders, fallback parsers, compatibility adapters, optional
  validators, or hand-maintained copies of upstream field sets.

## Non-goals

- Changing configured monitor values or enabling monitors.
- Expanding supported providers, credentials, or execution capabilities.
- Changing strategy behavior beyond closing the registration type boundary.
- Completing the remaining scope of issue #1383.

## Considered approaches

### 1. Patch the reported instances

Remove the two overridable methods, add OKX `region` to the allowlist, and add
the missing tests. This is the smallest textual change, but it preserves the
duplicated schema authority that caused the OKX drift. Rejected.

### 2. Generate Bolt allowlists from upstream config types

Generate or fence the field lists against NautilusTrader. This would close the
immediate drift but create a second schema representation and additional build
machinery. It would also preserve two parsing authorities. Rejected.

### 3. Close both classes at their boundaries

Make the registry own all erased construction and registration functions, and
let each pinned NautilusTrader config type own its accepted field schema.
Bolt's shared parse boundary enforces the product-specific credential policy;
the OKX implementation additionally requires the three monitor controls, and
the Kraken implementation invokes the official invariant validator. Selected.

## Design

### Non-overridable strategy registration

`StrategyBuilder` exposes only:

- its associated concrete `Strategy` type;
- `kind`;
- `validate_config`; and
- `build_typed`.

The trait will not expose erased `build` or `register` methods. Private generic
functions owned by the registry will:

1. call `B::build_typed`;
2. erase the returned `B::Strategy` for non-runtime inspection; or
3. derive its `StrategyId` and pass that exact value to
   `Trader::add_strategy`.

`register_guarded<B>` will require `B::Strategy: FillVoidPolicyGuard` before it
stores those registry-owned function pointers. There is no builder override
surface between the bound and the value registered with NautilusTrader.

### Single provider schema authority

Delete `BITMEX_DATA_FIELDS`, `BYBIT_DATA_FIELDS`,
`COINBASE_DATA_FIELDS`, `DERIBIT_DATA_FIELDS`, `OKX_DATA_FIELDS`, and
`KRAKEN_DATA_FIELDS`, plus the helpers that compare TOML keys against them.

`DataConfigBoundary::parse` remains the single typed path used by startup
validation and adapter mapping. Before provider deserialization, one shared
Bolt helper rejects direct credential keys. This preserves the SSM-only policy
at both boundaries. The pinned NautilusTrader config's
`#[serde(deny_unknown_fields)]` then owns all ordinary field acceptance.

The two provider-specific policy implementations remain type-directed:

- OKX first deserializes `RequiredOkxBookHealthControls`, then the official
  `OKXDataClientConfig`, and copies the three required values into the upstream
  type because its own serde defaults would otherwise make them optional.
- Kraken deserializes `KrakenDataClientConfig` and invokes its official
  `validate()` method.

These are properties of the relevant config types, not runtime venue-name
conditionals or fallback branches.

### Fail-closed evidence

Targeted tests will demonstrate:

- every new Hyperliquid monitor key is rejected when omitted;
- Polymarket `drop_quotes_missing_side` is rejected when omitted;
- every OKX monitor key remains rejected at both validation and mapping;
- Kraken Spot plus Demo is rejected by the shared typed boundary at both
  validation and mapping;
- OKX `region` is accepted and reaches the official config type; and
- direct credential keys and unknown fields remain rejected at both validation
  and mapping after removal of the manual allowlists.

The registry's structural proof is compilation: after removal of the trait
methods, a builder cannot override erased construction or registration. Tests
continue to exercise construction and runtime registration through stored
registry entries.

## Error handling

- Missing `[data]` remains an error at validation and mapping.
- Direct credentials produce the existing SSM-policy error during validation
  and a schema-mapping error during direct adapter mapping.
- Upstream serde errors, required OKX control errors, and Kraken validation
  errors are converted into the existing boundary-specific error types.
- No failed parse can produce an empty or partially configured adapter.

## Verification

Local verification will follow the repository's remote-first policy:

- formatting check;
- targeted Python/static source fences applicable to the changed boundary;
- workflow/config hygiene gates if affected;
- source inspection confirming the six allowlists and overridable trait methods
  are absent; and
- exact-head remote Rust verification through the governed PR workflow.

External review will be requested only after the fixes are committed, pushed,
the working tree is clean, local evidence is recorded, and the required
exact-head pre-cutover checks are green.
