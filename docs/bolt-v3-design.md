# bolt-v3 Architecture Specification

Status: draft for architecture review

This document supersedes the previous `docs/bolt-v3-design.md` draft.

The previous draft mixed multiple incompatible designs:

- mixin composition
- subprocess-based secret resolution
- wave-2 deploy signing and versioned releases
- a bolt-owned executable-order schema
- older issue-thread decisions that were later reopened

This document is the current proposed architecture after the reset.
It is intentionally explicit. If a behavior is important, it should be written here rather than inferred later in code.

## 1. Purpose

`bolt-v3` is a thin pure-Rust assembly binary on top of NautilusTrader.

It is not:

- a trading platform beside NautilusTrader
- a rules engine
- a selector framework
- a custom data plane
- a second order model
- a strategy runtime separate from NautilusTrader

It exists to do only the work NautilusTrader does not already do for this application:

- load operator-owned TOML config
- resolve secrets from Amazon Web Services Systems Manager
- assemble NautilusTrader nodes and clients
- register local Rust strategies
- provide minimal deploy/startup/runtime glue
- provide minimal strategy-decision forensics where NautilusTrader does not already know the "why"

Everything else belongs to NautilusTrader or should not exist.

## 2. Hard Constraints

These are architectural constraints, not implementation suggestions.

1. No hardcodes. Every runtime value comes from TOML.
2. Fail loud.
3. Single source of truth.
4. Trading latency near-zero.
5. Amazon Web Services Systems Manager is the single secret source.
6. Pure Rust binary. No Python, no PyO3, no pip.
7. Solo non-technical operator. Prioritize simplicity.
8. Build only what is required for correctness. Defer quality-of-life.
9. No dual paths.
10. No strategy-specific hacks and no market-family-specific hacks in the layer outside explicitly declared typed first-live target support.
11. If NautilusTrader already provides it, use NautilusTrader rather than rebuilding it.

## 3. Reset Rule

`bolt-v2` is a donor repository, not a base to preserve.

The default rule is:

- delete everything unless it earns its way back because NautilusTrader does not already provide it, or because the hard constraints require it, or because first-live-trade correctness requires it

This rule applies to:

- code
- configuration shape
- runtime behavior
- operational machinery
- debugging and logging surfaces

## 4. First-Live-Trade Scope

The first-live-trade slice is intentionally narrow.

Required before first live trade:

- Polymarket as the only execution venue
- one root/entity TOML plus separate strategy TOMLs
- no mixins and no config composition
- root file explicitly lists strategy files
- bounded typed target resolution
- no broad market discovery
- one shared process per trader instance
- strategy-to-execution boundary uses NautilusTrader-native order types directly
- `just check` as the canonical validation path
- artifact verification and controlled deploy/startup path
- panic issue `#239` resolved by test and explicit acceptance

Not blockers before first live trade:

- NautilusTrader paper trading
- NautilusTrader backtesting
- Kalshi
- `one_touch`
- Amazon Simple Storage Service archival

Immediate follow-up sequence after first live trade:

1. Full NautilusTrader backtesting
2. `one_touch`
3. Amazon Simple Storage Service archival of forensic artifacts
4. Dedicated NautilusTrader pointer-update slice, treated as a standalone change with re-verification of config mapping, target loading/filter behavior, custom-data persistence, and issue `#239` if the pin changes
5. Kalshi only when a real NautilusTrader-style adapter path exists

Reference-data venues may also be declared in first-live scope if the shipped strategy archetype requires them.
That does not change the rule that Polymarket is the only execution venue in the first-live slice.

## 5. Non-Goals

These are explicitly out of scope for first live trade.

- mixins
- inheritance
- generic selector expression languages
- regex or expression-string filters
- platform defaults schema-evolution framework
- hot reload
- bolt-owned order schema
- bolt-owned intent-to-order translation layer
- reference-price actor or reference-runtime framework
- normalized sink or custom data plane
- bolt-owned execution state tracker
- custom backtest engine
- custom paper-trading runtime
- Kalshi-shaped placeholder schema

## 6. Ownership Boundary: NautilusTrader vs bolt-v3

### NautilusTrader owns

- event loop
- lifecycle
- order model
- order submission and routing
- execution engine
- risk engine
- portfolio
- cache
- data engine
- built-in events
- built-in logging
- message bus
- live reconciliation
- venue adapters
- market/instrument objects
- backtest engine
- sandbox / paper mode when wired

### bolt-v3 owns

- operator TOML schema
- root/strategy file loading
- Amazon Web Services Systems Manager secret resolution
- environment-variable fallback blocking for secrets
- keyed venue configuration loading
- strategy registration by archetype string
- typed target configuration
- target resolution policy
- minimal decision-event schema for strategy rationale
- deploy/startup policy outside the trading path

### bolt-v3 does not own

- a second order schema
- a second risk engine
- a second portfolio
- a second execution layer
- a second message bus
- a second persistence system

## 7. High-Level Runtime Flow

The intended flow is:

1. Operator edits one root TOML and one or more strategy TOMLs.
2. Operator or automation runs `just check`.
3. Structural phase validates schema, ownership boundaries, references, and local semantics.
4. Live phase validates secret resolution, environment assumptions, target resolution, and node assembly using the same underlying code paths.
5. Deploy automation installs a versioned release, verifies artifacts, flips the release pointer, and starts or restarts the service.
6. `bolt` loads the root file and the explicitly listed strategy files.
7. `bolt` resolves Amazon Web Services Systems Manager references and fails loud if any required secret is missing.
8. `bolt` assembles NautilusTrader data and execution clients from keyed venue configuration.
9. `bolt` registers local Rust strategies.
10. Each strategy resolves its own configured target in `on_start` and via explicit retry timers where required.
11. Strategies subscribe directly to NautilusTrader data clients for trading data and any declared reference data.
12. Strategies construct NautilusTrader-native orders directly and submit them through NautilusTrader.
13. NautilusTrader routes those orders to the correct venue adapter.
14. Polymarket-specific wire translation happens inside NautilusTrader's Polymarket adapter, not in bolt.
15. Strategy rationale is emitted as minimal structured decision events through NautilusTrader-native mechanisms, with logs as a readable mirror.

## 8. File Model

The file model is fixed for first live trade.

### Root/entity file

One root file controls one running process.

It owns:

- canonical trader identity
- runtime mode
- NautilusTrader node settings
- entity-level risk settings
- logging settings
- persistence paths
- keyed venue definitions
- venue secret references
- explicit list of strategy files

### Strategy file

One strategy file defines one strategy instance.

It owns:

- strategy instance identity
- strategy archetype
- venue reference
- target definition
- target retry/block behavior
- optional reference data declarations
- archetype-specific parameters
- archetype-specific order-construction parameters

### Ownership rule

The rule is:

- no overlap in authority
- references allowed
- duplicate ownership forbidden

Examples:

- root owns the Polymarket execution client and secret references; strategy may reference the keyed venue name but may not redefine its credentials
- strategy owns its target resolution timing; root may not override it globally

## 9. Canonical Identity Rule

There is one canonical process/trader identity in the root schema:

- `trader_identifier`

This value is used for:

- NautilusTrader `TraderId`
- service/process identity in runtime forensics
- local state namespace
- log/event identity where a trader-level identifier is needed

The root schema does not carry a separate `entity_identifier` or `node_name`.

For first live trade:

- Nautilus node name is set equal to `trader_identifier`

This is a deliberate fixed mapping, not an extra configurable identity.

Venue keys such as `polymarket_main` are keyed configuration names, not trader identity.

## 10. Root/Strategy Reference Model

Keyed venues exist to support both trading venues and data-only venues.

The distinction is mechanical:

- a venue block with `[data]` only is data-only
- a venue block with `[execution]` only is execution-only
- a venue block with both supports both

A strategy's `venue` field must reference a keyed venue that has execution when the strategy trades on it.

Reference data venues are declared separately and referenced explicitly by the strategy.

There is no hidden "primary trading venue" convention outside these references.

## 11. Target Model

The target model is bounded and typed.

First-live-trade supported target kinds:

- exact instrument
- bounded rotating `updown` series

Series support is intentionally narrow:

- `series_family = "updown"`
- `underlying_asset = "BTC"` or `"ETH"`
- `cadence_seconds = 300`
- `rotation_policy = "active_or_next"`

`one_touch` is not in the first-live-trade slice.
It is intentionally deferred until after full NautilusTrader backtesting.

For first live trade, `updown` support is explicit typed target support, not a hidden generic selector pretending to be venue-neutral.

## 12. Series Resolution Model

The series resolver is not a separate runtime subsystem.

Rules:

- each strategy owns the lifecycle of resolving its own target
- there is no standalone selector service, actor, or framework
- shared pure helper functions are allowed
- shared runtime state or a parallel resolver runtime is not allowed

Resolution source of truth:

1. NautilusTrader-loaded instrument and venue state
2. one narrow Polymarket Gamma supplement for first-live `updown` anchor extraction only

The supplement exists because current NautilusTrader Polymarket models do not expose Gamma `eventMetadata.priceToBeat`.
It is not a discovery path, not a data plane, and not available to strategies directly.

For first-live `updown`, bolt installs a dynamic NautilusTrader `MarketSlugFilter` closure and uses NautilusTrader's `request_instruments` path at startup and when the current/next slug pair changes.
That keeps rolling target refresh inside NautilusTrader's data-client path rather than inventing a bolt-side discovery loop.

`active_or_next` means:

- use the currently active tradable market if exactly one exists
- otherwise use the next tradable market in the same declared series if exactly one exists
- otherwise fail loud for that attempt
- do not guess
- do not broaden search
- do not select fallback markets outside the declared series

Retry policy for unresolved or ambiguous series targets:

- retry at the configured strategy `retry_interval_seconds`
- no backoff
- remain non-trading while unresolved
- after the configured `blocked_after_seconds` unresolved interval, mark the strategy blocked/degraded
- continue retrying until the strategy is stopped or resolution succeeds

These retry/block values are strategy-owned target behavior, not root-owned process behavior.

## 13. NautilusTrader Pin

The first-live-trade design targets NautilusTrader release `v1.225.0`:

- `48d1c126335b82812ba691c5661aeb2e912cde24`

This is not advisory context. It is part of the runtime contract.
`Cargo.toml` must be updated to this exact revision before vertical reviews proceed.

If the pin changes:

- the mapping from TOML to NautilusTrader config must be re-reviewed
- the Polymarket adapter surface must be re-verified
- the panic gate in issue `#239` must be re-run for the new pin

Verified baseline facts for this pin:

- NautilusTrader does not expose Gamma `eventMetadata.priceToBeat`; bolt owns the narrow first-live Gamma supplement for that field only.
- NautilusTrader Polymarket and Binance configs both have environment-variable credential fallbacks when credentials are omitted; bolt must block those before client construction.
- NautilusTrader does not expose a single strategy-facing `sellable_quantity` helper for this pin; first-live sellable quantity must be derived from NT position state minus open sell leaves without a custom execution tracker.
- NautilusTrader `v1.225.0` is V1-signed for Polymarket CLOB; Polymarket CLOB V2 migration remains an explicit first-live blocker to resolve before live capital.

## 14. Reference Data Model

If a strategy archetype requires external reference data, it must declare it explicitly in the strategy file.

Rules:

- reference data must not be hardcoded inside the archetype
- reference data venues must be declared in the root file as keyed data-only or data-capable venues
- strategies subscribe directly to those instruments through NautilusTrader data clients
- there is no bolt-owned reference actor or reference runtime

## 15. Strategy / Execution Boundary

This is a critical correction from earlier drafts.

There is no bolt-owned executable-order schema.
There is no bolt-owned intent-to-order translation layer.

The strategy-to-execution boundary uses NautilusTrader-native order types directly.

That means:

- strategies construct NautilusTrader `Order` values or use NautilusTrader order-factory helpers
- NautilusTrader execution engine routes those orders
- the venue adapter translates NautilusTrader order semantics into venue-native wire semantics
- bolt does not define another order struct in the middle

If a Polymarket-specific execution gap exists which NautilusTrader's order model cannot express cleanly:

- the preferred fix is upstream in the NautilusTrader Polymarket adapter
- not a bolt wrapper around that boundary

This rule is what prevents bolt-v3 from recreating bolt-v2's higher-level intent translation layer.

## 16. Order Authority Rules

Because the strategy uses NautilusTrader-native orders directly, the rules for order authority are:

- venue wire translation belongs to NautilusTrader venue adapters
- venue precision enforcement belongs to NautilusTrader and the venue adapter wherever possible
- strategy-local remembered quantities are never authoritative for exits
- position and sellable state must come from NautilusTrader cache and venue-confirmed state
- local pre-submit checks must fail loud if a quantity or price is invalid before network submit

bolt may add minimal local validation only where NautilusTrader or the adapter does not already catch the condition early enough.

## 17. Validation Path

There is one canonical validation path:

- `just check`

`just check` must call one underlying validation implementation.
There is no second validation logic path in the `justfile`.

Validation output is split into two explicit phases:

- structural/config validation
- live/environment validation

This split is for operator clarity only.
It is not two different architectures.

If the current market state does not yield a resolvable `active_or_next` target, the live phase reports that loudly as an operational warning rather than a fatal validation error.
That matches the first-live runtime design, where unresolved rotating targets remain non-trading and retry rather than forcing process exit.

## 18. Observability and Forensics

The observability rule is:

- NautilusTrader-native observability first
- bolt must not build a parallel logging, metrics, or data plane
- bolt may add only minimal decision-event linkage where NautilusTrader does not know the strategy's rationale

The forensic goal is reconstructability, not "more logs."

For first live trade:

- strategies emit a small fixed set of structured decision event types
- every event carries a fixed common field set
- each event type carries a fixed event-specific field set
- logs mirror those events for readability
- there is no generic event framework

Amazon Simple Storage Service archival is not required before first live trade.
It is an immediate follow-up.

## 19. Deployment and Runtime Controls

Required before first live trade:

- artifact verification before startup
- versioned release directories
- atomic release-pointer switch for deploy and rollback
- one automated deploy/restart path only
- restart permission limited to the deploy identity
- crash dumps disabled
- restart loops capped
- writable filesystem paths explicitly allow-listed

These controls are operational.
They are not part of the trading layer.

## 20. Panic Behavior Gate

Issue `#239` is a hard blocker before first live trade.

Resolved means:

- panics are injected into startup, market-data, order-event, position-event, and timer callbacks
- tests run on the exact pinned release build
- process exit behavior, logs, and systemd behavior are recorded
- blast radius is documented
- resulting behavior is explicitly accepted before first live trade

Unknown panic behavior is not acceptable.

## 21. Immediate Follow-Up Work

Immediately after first live trade:

1. Full NautilusTrader backtesting wired through the same strategy code
2. `one_touch`
3. Amazon Simple Storage Service archival/export of forensic artifacts
4. Dedicated NautilusTrader pointer-update slice, treated as a standalone change with re-verification of config mapping, target loading/filter behavior, custom-data persistence, and issue `#239` if the pin changes
5. Kalshi only when a real NautilusTrader-style adapter path exists

## 22. Explicit Deletions From bolt-v2

The following classes of behavior are explicitly deleted or forbidden unless re-justified later:

- mixins
- config composition
- collision-detection merge engine
- ruleset engine
- platform runtime
- normalized sink
- bolt-owned executable-order schema
- bolt-owned intent translation layer
- reference actor / reference publisher
- dual config path
- raw capture transport in the trading runtime
- bolt-owned execution state tracker
- custom lake writers
- Kalshi-shaped placeholders

## 23. Supporting Documents

This architecture document is intentionally paired with:

- `docs/bolt-v3-schema.md`
- `docs/bolt-v3-runtime-contracts.md`
- `docs/bolt-v3-contract-ledger.md`

Those files carry the exact schema and runtime rules that this document references.
