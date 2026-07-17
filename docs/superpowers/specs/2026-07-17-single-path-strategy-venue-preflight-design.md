# Atomic Strategy Preparation And Single-Path Client Resolution Design

## Scope

PR #1442 implements issue #1431 B1, B2, and C2. Registration must reject every
deterministic configuration, client-resolution, registry-selection, and
strategy-construction failure before it mutates `LiveNode`. It must also retain
the existing single configured execution route, capability-gated resources,
and fail-closed unsided market-order admission behavior.

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

Shared preflight constructs one private resolved-client set per strategy. It
contains the configured venue for every declared client role and retains the
resolved execution client only long enough to derive execution venue, fee
provider, and settlement identity. It stores no resolved credentials.

`StrategyRegistrationContext` exposes a crate-visible read of a prepared client
venue for runtime binding preparation. Runtime preparation functions must not
read `loaded.root.clients`, call the neutral client-table accessors, or perform
a second provider lookup. Existing load-time validation may inspect root config
but cannot register a strategy or mutate `LiveNode`.

For settlement-capable strategies, settlement account and currency are derived
from the preflight-resolved execution client and venue before fee-provider
construction. Non-settlement strategies use the same execution-route
resolution but receive no settlement resources.

### One strategy construction path

`StrategyBuilder` has one concrete construction method. The registry wraps that
method to produce the shared registration boundary's
`PreparedStrategyRegistration`, which owns the concrete strategy behind a
private one-use commit interface. The prepared value contains:

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

### Prepare, validate, then commit

The dispatcher performs three ordered stages:

1. Resolve shared client routes and capability resources, then invoke each
   binding's pure preparation function.
2. Collect every `PreparedStrategyRegistration`; ask NT to prepare each ID
   without registering it, then reject duplicate prepared IDs, duplicate order
   ID tags, and IDs already present in the trader before mutation.
3. Commit the prepared strategies to NT in configured order and build the
   registration summary from the prepared identities.

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
| A binding defers parsing or construction until mutation | `StrategyRuntimeBinding` exposes prepare, not register; prepared commit owns a built strategy | Structural test rejects raw mapping, registry selection, or strategy building in the commit loop |
| Builder validation and registration diverge | One concrete `StrategyBuilder` construction method | Registry tests prove prepare constructs once and commit does not reconstruct |
| A duplicate or already-registered NT ID is discovered during commit | Prepared IDs and existing trader IDs are checked as complete sets before mutation | Duplicate-ID and existing-ID regressions assert zero new registrations |
| Settlement identity uses another route | Settlement consumes the prepared execution client and venue before fee-provider construction | Account/currency failures return typed errors and execute no commit |
| Secrets or capability handles leak to bindings | Resolved credentials are constructor-only; prepared route and capability fields stay private | Structural tests reject any stored `ResolvedBoltV3Secrets` and undeclared resource access |
| Missing configuration is silently replaced | Every absent client/account/currency is an error | Fail-closed tests cover each missing identity with no fallback or unwind |
| Documentation reintroduces the retired ordering | Design and implementation plan show settlement before fee provider and no `binding_message` wrapper | Targeted text checks and internal adversarial review |

## Verification

Use evidence-driven test-first verification:

1. Add the real-production-binding invalid-second-signal-client regression and
   confirm it fails because the first strategy is registered.
2. Add alias and duplicate-prepared-ID regressions before production changes.
3. Add structural checks that reject client-map reads in binding preparation,
   stored resolved secrets, deferred builder work, and mutating preflight.
4. Implement the single prepare/commit path and remove the old registration
   path rather than retaining an adapter or fallback.
5. Run governed local formatting, documentation checks, static source fences,
   and internal adversarial review.
6. Publish a clean exact head and use remote Rust compilation, Clippy, and
   behavior tests as execution evidence.

Human code-owner approval, last-push approval, and review-thread resolution
remain mandatory before queueing.
