# Atomic Strategy Preparation And Single-Path Client Resolution Design

## Scope

PR #1442 implements issue #1431 B1, B2, and C2. Registration must reject every
deterministic configuration, client-resolution, registry-selection, and
strategy-construction failure before it mutates `LiveNode`. It must also retain
the existing single configured execution route, capability-gated resources,
and fail-closed unsided market-order admission behavior.

Live registration and the production-registry branches in
`backtesting-vertical-slice` must use the same atomic prepared-batch
coordinator. Those Backtester branches may submit a batch of one, but they must
not retain a separate registry-to-`Trader::add_strategy` path.

B3 and bucket D remain excluded. The final NautilusTrader `add_strategy`
operation is a commit boundary, not a transaction supplied by this repository.
This design preflights every deterministic input to that operation; it does not
claim rollback of an NT failure that occurs while committing an already-built
strategy.

## Configuration Authority

Each configured client role has one source:

- execution: `strategy.execution_client_id`
- signal data: each `strategy.signal_data[*].data_client_id`
- resolution data: `strategy.resolution_data.data_client_id`, when present

During runtime strategy preparation, the root `clients` table is resolved only
by shared registration preflight. Load-time configuration validation remains a
separate, non-mutating gate. Client identifiers are deduplicated before runtime
lookup, so one client referenced by multiple roles is read once and reused. In
particular, a signal or resolution role that aliases the execution client
cannot reopen the root client map.

Production code must not contain a venue, client ID, account, currency,
default, or alternate-routing literal. Missing configuration is an error.
Runtime health may block execution at the configured venue, but it must never
select another venue.

## Architecture

### Prepared client routes

Shared preflight constructs one resolved-client set per strategy. It
contains the configured venue for every declared client role and retains the
resolved execution client only long enough to derive execution venue, fee
provider, and settlement identity. It stores no resolved credentials.

`StrategyRegistrationContext` does not store or expose `LoadedBoltV3Config`,
`BoltV3RootConfig`, `ClientBlock`, or `ResolvedBoltV3Secrets`. Runtime bindings
receive only the configured strategy, prepared client venues, and the narrow
non-client configuration snapshot described below. They cannot reopen the root
client map through the context, a raw mapper, or a cross-file helper.

The safe prepared-route type contains only the venue map and exposes lookup by
configured `ClientId`; it contains no `ClientBlock`. The internal resolver
retains raw client references only until Live preflight derives execution
resources, then returns the safe route value. A public wrapper around that same
resolver supplies Backtester mapping, including alias deduplication. Neither
consumer implements a second client-table traversal.

For settlement-capable strategies, settlement account and currency are derived
from the preflight-resolved execution client and venue before fee-provider
construction. Non-settlement strategies use the same execution-route
resolution but receive no settlement resources.

### Narrow non-client configuration snapshot

Edge-taker raw mapping needs realized-volatility policy, gate-provider
freshness, and Chainlink feed-binding information. Shared preflight copies only
those non-secret, non-client values into an immutable preparation snapshot.
The snapshot contains no root config, client table, client block, credentials,
account identity, or alternate routing information.

Runtime bindings and Backtester raw mapping consume this snapshot plus prepared
routes. Adding another raw-mapping dependency requires extending the explicit
snapshot rather than restoring access to the full loaded configuration.

### One strategy construction path

`StrategyBuilder` has one concrete construction method. The registry wraps that
method to produce the shared registration boundary's
`PreparedStrategyRegistration`, which owns the concrete strategy behind a
private one-use commit interface. The type is an opaque public return value so
the Backtester workspace can hold it, but it exposes no public constructor,
identity-preparation method, strategy accessor, or commit method. The registry
is its only producer and the shared batch coordinator is its only consumer.
The prepared value contains:

- the NT strategy ID after the dispatcher runs NT's non-mutating
  `prepare_strategy_for_registration` check; and
- a one-use commit operation owning the already-built concrete strategy.

The existing separate `build` and `register` implementations are removed. A
strategy cannot be parsed or constructed once for validation and again for
registration.

Each `StrategyRuntimeBinding` provides a pure preparation function rather than
a mutating registration callback. Binding preparation performs every fallible
binding-specific operation: raw-config mapping, build-context assembly,
registry selection, config parsing, and concrete strategy construction. It
receives no `LiveNode` and cannot mutate the trader.

The Backtester's production-registry branches use the same registry preparation
method. Those branches do not receive a compatibility `register_strategy`
adapter and do not call `add_strategy` directly; unrelated Backtester-only
strategy kinds remain outside this PR's declared scope.

### Prepare, validate, then commit

The shared batch coordinator performs the final two stages for Live and the
affected Backtester production-registry branches.
Live registration performs four ordered stages:

1. Resolve shared client routes and capability resources, then invoke each
   binding's pure preparation function.
2. Collect every `PreparedStrategyRegistration` without trader mutation.
3. In the shared coordinator, ask NT to prepare each ID
   without registering it, then reject duplicate prepared IDs, duplicate order
   ID tags, and IDs already present in the trader before mutation.
4. In that same coordinator, commit the prepared strategies to NT in input
   order and return their prepared NT strategy IDs. Live registration builds
   its richer summary from those IDs and its configured strategy metadata.

Each affected Backtester branch performs its own non-mutating manifest/config
preparation, then passes its registry-produced batch—normally one strategy—to
stages 3 and 4. There is one NT commit implementation for production-registry
strategies.

No mutating callback runs during stages 1 or 2. A missing signal client,
missing resolution client, malformed raw strategy config, unsupported registry
entry, duplicate strategy ID, or concrete strategy-construction error in any
configured strategy therefore leaves the trader unchanged.

## Error Handling

All preparation failures map to the existing strategy-specific `Binding`
error, preserving strategy instance and archetype identity. Client-role errors
identify the configured role and client ID without substituting a default.
Unknown currency remains fallible and panic-free.

The final commit operation may still return an NT error. Because all
deterministic repository-owned work and duplicate-ID checks have already
succeeded, that error represents the explicit external commit boundary rather
than deferred configuration validation.

## Risk Controls And Evidence

| Risk | Structural control | Required evidence |
| --- | --- | --- |
| A later invalid strategy leaves earlier registrations behind | Collect all concrete prepared strategies before commit | A valid first edge strategy plus a second strategy with a missing signal client produces zero callbacks and zero registrations |
| A client role reopens the root client map | One shared identity-deduplicating client resolver; prepared route reads thereafter | Alias execution and signal client IDs and prove one root-map read and correct preparation |
| A binding launders a late lookup through `context.loaded` or a helper | Context contains no loaded/root/client-block reference; raw mapping receives only routes and a non-client snapshot | Compile/API checks and source fences reject loaded/root/client types and client lookup provenance in preparation callbacks |
| A binding defers parsing or construction until mutation | `StrategyRuntimeBinding` exposes prepare, not register; prepared commit owns a built strategy | Structural test rejects raw mapping, registry selection, or strategy building in the commit loop |
| Builder validation and registration diverge | One concrete `StrategyBuilder` construction method | Registry tests prove prepare constructs once and commit does not reconstruct |
| External code bypasses the batch checks | Prepared values expose no public constructor/prepare/commit method; one shared coordinator is the sole consumer | API/compile tests prove direct prepare/commit is unavailable and Live plus the affected Backtester production-registry branches use the coordinator |
| A Backtester production-registry branch retains the deleted registration route | Those branches use shared route/snapshot preparation, registry preparation, and the common batch coordinator | Backtester Clippy/archive compile and structural checks reject `register_strategy` and direct `add_strategy` in those branches |
| A duplicate or existing NT strategy ID/order tag is discovered during commit | Prepared IDs/tags and existing trader IDs/tags are checked before mutation | Duplicate-batch and existing-tag regressions assert zero new registrations |
| Settlement identity uses another route | Settlement consumes the prepared execution client and venue before fee-provider construction | Account/currency failures return typed errors and execute no commit |
| Secrets or capability handles leak to bindings | Resolved credentials are constructor-only; prepared route and capability fields stay private | Structural tests reject any stored `ResolvedBoltV3Secrets` and undeclared resource access |
| Missing configuration is silently replaced | Every absent client/account/currency is an error | Fail-closed tests cover each missing identity with no fallback or unwind |
| Documentation reintroduces the retired ordering | Design and implementation plan show settlement before fee provider and no `binding_message` wrapper | Targeted text checks and internal adversarial review |

## Verification

Use evidence-driven test-first verification:

1. Add the real-production-binding invalid-second-signal-client regression and
   confirm it fails because the first strategy is registered.
2. Add missing-resolution-client, alias, duplicate-prepared-ID, and existing
   trader order-tag regressions before production changes.
3. Add structural checks that reject loaded/root/client-block reachability,
   client-map reads in binding preparation, stored resolved secrets, deferred
   builder work, public direct commit methods, and identity preparation inside
   the commit loop. Run those checks on comment/string-stripped source.
4. Implement the safe route/snapshot boundary and the single shared batch
   coordinator. Migrate the affected Backtester production-registry branches
   rather than retaining an adapter or fallback.
5. Compile-check registry tests, Live wiring, and the Backtester workspace in
   the governed remote lanes; warnings are errors and must not be suppressed.
6. Run governed local formatting, documentation checks, static source fences,
   and internal adversarial review.
7. Publish a clean exact head and use remote Rust compilation, Clippy, and
   behavior tests as execution evidence.

Human code-owner approval, last-push approval, and review-thread resolution
remain mandatory before queueing.
