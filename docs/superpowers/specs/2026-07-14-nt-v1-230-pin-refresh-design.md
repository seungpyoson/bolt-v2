# NT v1.230 Pin Refresh Design

**Date:** 2026-07-14

## Decision

Refresh Bolt's NautilusTrader dependency from the current `0.59.0` pin-fork
revision, `afc014a55b51463641cc19c68bffe25cdac6588a`, to a new immutable fork
revision based on the official NautilusTrader `v1.230.0` release commit,
`8160730c7c550480b0a439fb11086a4c4de15f0b`.

The new fork revision will preserve the three Binance Spot SBE behaviors Bolt
requires:

1. schema 3:5 instrument loading from upstream commit
   `9a2e7a5155ffaa515c0279951eb1a06a8652ca33`;
2. schema 3:5 request negotiation from upstream commit
   `3b59b08c9e651075a462f243c01664f4ccbd9b21`;
3. distinct adapter receive-clock ownership for `ts_init`, currently carried by
   fork commit `fa3391d90c1aace4733fc73dae082b4cfee6b8fa`.

The final dependency pointer will be the exact 40-character SHA produced after
those patches and their focused tests are applied to the release base. Bolt will
not depend on a movable branch or tag.

## Why This Revision

NautilusTrader documents `master` as the production-oriented release line and
`develop` as active development. The latest official release is `v1.230.0`.
Using that release gives Bolt the fixes and Rust API improvements shipped in
`v1.228.0` through `v1.230.0` without accepting the additional unreleased API
and MSRV drift on current `develop`.

The official release cannot be used unchanged. It predates complete Binance
Spot SBE schema 3:5 support and does not preserve Bolt's required distinction
between venue event time (`ts_event`) and adapter initialization time
(`ts_init`). A release-based fork revision with the minimum required patches is
therefore the smallest production-oriented upgrade.

## Alternatives Considered

### Pin current upstream `develop`

This would include the newest Polymarket, Binance, Hyperliquid, reconciliation,
and LiveNode fixes. It also moves NT crates from `0.59.0` to `0.61.0`, raises
the Rust MSRV from `1.96.0` to `1.97.0`, contains hundreds of unreleased
commits, and still lacks Bolt's SBE receive-clock patch. That blast radius is
not justified for a live-trading dependency refresh.

### Keep the current fork revision

This preserves known behavior and requires no compatibility work, but remains
on an intermediate `0.59.0` development revision and misses the production
fixes released in `v1.229.0` and `v1.230.0`.

### Use official `v1.230.0` without patches

This is the narrowest upstream pin, but it regresses the Binance Spot SBE 3:5
and receive-clock contracts already required and source-fenced by Bolt.

## Scope

The slice includes only work needed to create and consume the release-based NT
revision:

- create an immutable commit in the Bolt NT fork from official `v1.230.0`;
- port the minimum SBE 3:5 loading, request, receive-clock, fixture, and focused
  test changes required by the three behavior contracts above;
- update every governed Bolt Cargo manifest and lockfile to the resulting SHA;
- make only Bolt source adaptations proven necessary by NT `0.60.0` compile or
  contract evidence;
- update the runtime-contract, provenance, audit, test-fixture, and boundary
  evidence surfaces that intentionally record the dependency revision;
- verify compatibility and fail-closed behavior through local non-compile gates
  and exact-head remote Rust CI.

The slice does not add trading features, change strategy behavior, alter TOML
runtime values, enable live trading, or adopt unrelated post-`v1.230.0`
development work.

## Change Flow

1. Create a dedicated branch in `seungpyoson/nautilus_trader` at the official
   `v1.230.0` release commit.
2. Port the two upstream SBE 3:5 changes and Bolt's receive-clock change. Resolve
   conflicts against the release source without bringing unrelated commits.
3. Run focused NT adapter formatting and tests. Commit the coherent patch set to
   obtain the immutable candidate SHA.
4. Replace the old revision in Bolt's root and backtesting-vertical-slice
   manifests, then regenerate both lockfiles through the governed dependency
   workflow.
5. Apply only compile or contract adaptations caused by the NT `0.60.0` API.
6. Update all authoritative pin evidence to the same candidate SHA.
7. Run local static gates, publish the Bolt branch, and use exact-head remote
   verification for Rust compilation, tests, clippy, and dependency checks.

## Required Behavior

The upgrade is acceptable only if all of the following remain true:

- Binance Spot SBE requests and instrument loading use schema 3:5.
- Each decoded SBE WebSocket message captures one adapter clock value and uses
  it as `ts_init` for trades, BBO quotes, snapshot aggregates and inner deltas,
  and diff aggregates and inner deltas.
- Venue-provided timestamps remain `ts_event`; Bolt performs no restamping or
  inference.
- The Polymarket CLOB V2 readiness evidence remains valid against the new pin.
- Panic-gate, credential, config, admission, and provider-boundary checks remain
  fail-closed.
- Root and backtesting manifests and lockfiles resolve every `nautilus-*`
  package to one fork URL and one exact revision.

## Error Handling

If either upstream SBE change cannot be isolated cleanly on `v1.230.0`, the NT
fork work stops before Bolt's pointer changes. If the candidate compiles but a
governed behavior check regresses, the pointer is not accepted and the failure
is fixed in the dedicated NT patch set or in a minimal Bolt compatibility
change. No alternate dependency path, temporary branch pointer, ignored test,
or live-risk waiver is allowed.

## Verification Evidence

The implementation plan will map each changed requirement to evidence. At
minimum it will require:

- focused NT Binance SBE parser and private-handler tests for schema 3:5 and
  `ts_event`/`ts_init` separation;
- Bolt local formatting, Python verifier, workflow-lint, and
  `just source-fence-static` checks allowed by repository policy;
- pin-census proof that both manifests and both lockfiles resolve to the same
  exact fork revision;
- the CLOB V2 readiness and panic-gate matrices required by the runtime
  contract;
- targeted text/static checks and an internal adversarial review of changed
  documentation, verifier, and policy evidence;
- exact-head remote Rust CI on a draft PR, followed by green ready-PR merge
  proof and the required code-owner review before completion or merge.

No live operation is part of this dependency update.
