# bolt-v2 NT-Native Rust Architecture

## Status

Approved planning baseline for `bolt-v2`.

This document defines the thinnest viable architecture for `bolt-v2` on top of NautilusTrader Rust-live at:

- Repo: `https://github.com/nautechsystems/nautilus_trader`
- Commit: `af2aefc24451ed5c51b94e64459421f1dd540bfb`

## Core Decision

`bolt-v2` will be a thin NT-native Rust bootstrapper, not a second trading framework.

It will:

1. Load configuration.
2. Resolve secrets.
3. Construct NT config and client objects.
4. Register NT data clients.
5. Register NT execution clients.
6. Register NT strategies.
7. Run one `LiveNode`.

It will not build:

- a custom OMS
- a custom router
- a custom control plane
- a custom reconciliation layer
- a platform-level leg model
- a platform-level generic data requirement model
- a platform-level account registry

## Goals

- Pure Rust only.
- Use as much NT Rust-live functionality as possible.
- Minimize custom code on our side.
- Preserve long-run support for:
  - multi-venue
  - multi-instrument
  - multi-strategy
  - cross-venue reads
  - later multi-venue execution
  - later opportunity discovery / auto-matching

## Non-Goals

- No Python or PyO3 path.
- No bolt-v1 reuse.
- No custom trading platform abstraction over NT.
- No speculative config sections for features we are not yet using.

## Source-Backed Baseline

The architecture relies on these NT Rust-live facts at the pinned commit:

- Multiple data and execution clients can be registered on a live node.
- Multiple strategies can be registered on a live node.
- Pure-Rust execution algorithms exist.
- Local order emulation is wired into the Rust live kernel.
- The generic NT data surface is broad and includes quotes, trades, books, bars, mark/index/funding, status/close, and custom data at the engine/actor level.
- Execution routing is venue-based inside the execution engine.
- The execution engine rejects a second routed execution client for the same venue inside one node.

Important implication:

- Multi-venue execution is supported in one node.
- Same-venue multi-account execution is not a same-node assumption in this design.
- If needed later, same-venue multi-account should be treated as a multi-node or multi-process deployment concern.

Adapter coverage varies.

Example:

- Polymarket provides quotes, book deltas, trades, and related market-specific events.
- This does not imply venue-native support for every generic NT data type.
- Features such as bars may still come from NT internal facilities rather than direct adapter emission.

## Architecture

### Process Model

- One `bolt-v2` process corresponds to one NT `LiveNode`.
- One `LiveNode` may host:
  - multiple data clients
  - multiple execution clients
  - multiple strategies
- The process should remain thin and should not introduce a second orchestration layer above NT.

### Runtime Ownership

NT owns:

- client routing
- order routing
- OMS state
- reconciliation
- portfolio state
- risk engine behavior
- order emulation
- execution algorithm infrastructure
- data subscriptions and message bus dispatch

`bolt-v2` owns:

- config parsing
- secret resolution
- adapter/client factory selection
- strategy factory selection
- strategy implementations
- later missing venue adapters such as `Kalshi` or `HIP-4`

### Config Surface

The minimal required top-level config sections are:

- `[node]`
- `[logging]`
- `[[data_clients]]`
- `[[exec_clients]]`
- `[[strategies]]`

Optional sections should only be added when actually used:

- `[exec_engine]`
- `[cache]`
- `[portfolio]`
- `[msgbus]`
- `[streaming]`
- `[risk_engine]`

The minimal platform schema should not contain:

- top-level `accounts[]`
- top-level `wallet`
- top-level `signal_legs[]`
- top-level `execution_legs[]`
- top-level `data_requirements[]`
- top-level `validators`

Reason:

- These are either not NT first-class concepts in the builder/live-node path, or they belong inside strategy-specific config and code.

`[[actors]]` and `[[exec_algorithms]]` are real NT concepts, but they are not required in the minimal schema today.

They should be added when we actually use them, not before.

### Strategy Boundary

Strategy-specific complexity stays inside strategies.

Examples:

- cross-venue read logic
- arbitrage logic
- market-making logic
- market-family rotation for rolling or expiring instruments
- signal leg models
- execution leg models
- custom data requirements
- opportunity discovery logic

The top-level platform should not attempt to standardize those concepts.

Instead:

- `[[strategies]]` selects a strategy type.
- `[strategies.config]` carries strategy-specific opaque config.
- The strategy implementation decides how to subscribe, interpret, and trade.

Lifecycle note:

- Prediction markets and any other expiring or rolling instruments may require strategy-owned rotation or rollover logic.
- Perpetual derivatives usually do not need this.
- Dated futures, options, and prediction-market families often do.
- This remains a strategy concern, not a platform-level schema concern.

### Credentials

Credentials are per execution client, not global.

Therefore:

- no top-level `wallet`
- secrets live under `[[exec_clients]]`

This keeps the config structurally correct for multi-venue setups with different credential sets.

## Minimal Config Shape

```toml
[node]
name = "BOLT-V2-001"
trader_id = "BOLT-001"
environment = "Live"
load_state = false
save_state = false
timeout_connection_secs = 60
timeout_reconciliation_secs = 30
timeout_portfolio_secs = 10
timeout_disconnection_secs = 10
delay_post_stop_secs = 10
delay_shutdown_secs = 5

[logging]
stdout_level = "Info"
file_level = "Debug"

[[data_clients]]
name = "POLYMARKET"
type = "polymarket"

[data_clients.config]
subscribe_new_markets = false
update_instruments_interval_mins = 60
ws_max_subscriptions = 200
# adapter-specific market selection goes here when needed

[[exec_clients]]
name = "POLYMARKET"
type = "polymarket"

[exec_clients.config]
account_id = "POLYMARKET-001"
signature_type = 2
funder = "0x..."

[exec_clients.secrets]
region = "eu-west-1"
pk = "/bolt/polymarket/private-key-b64"
api_key = "/bolt/polymarket/api-key"
api_secret = "/bolt/polymarket/api-secret"
passphrase = "/bolt/polymarket/api-passphrase"

[[strategies]]
type = "my_strategy"

[strategies.config]
strategy_id = "MY-STRAT-001"
```

The example intentionally avoids assuming a specific market-selection scheme.

Market selection should remain adapter-specific inside client config or strategy config, for example:

- exact instrument IDs
- venue-specific selectors
- later discovery templates

## Smallest Module Set

`bolt-v2` should contain only these modules:

- `config`
- `secrets`
- `clients`
- `strategies`
- `main`

Responsibilities:

- `config`
  - parse TOML
  - hold the thin wrapper structs
- `secrets`
  - resolve SSM paths
  - inject or pass resolved values to adapter configs
- `clients`
  - map `type` to NT factories and adapter config construction
  - build filter objects where needed
- `strategies`
  - map `type` to Rust strategy constructors
  - keep strategy-specific config local to the strategy
- `main`
  - wire everything together
  - create the node
  - register clients and strategies
  - run the node

## Implementation Shape

The preferred runtime path is:

1. Parse TOML.
2. Resolve secrets.
3. Create `LiveNode::builder(...)`.
4. Apply required builder fields from `[node]` and `[logging]`.
5. Register `[[data_clients]]`.
6. Register `[[exec_clients]]`.
7. Build the node.
8. Apply optional post-build subsystem configuration for NT surfaces the builder does not expose directly.
9. Register `[[strategies]]`.
10. Run the node.

This is configuration glue, not a planner or orchestration layer.

Important nuance:

- The minimal path should prefer the builder because it matches NT's simple Rust-live registration flow.
- Optional subsystem tuning may still require post-build mutation or targeted glue code.
- `bolt-v2` should accept small amounts of awkward NT-native wiring instead of creating a new abstraction layer to smooth it over.

Builder-first vs post-build:

- The builder should handle the required happy path:
  - node identity
  - logging
  - core timeouts
  - client registration
- Optional subsystem tuning may require post-build mutation because the builder does not expose every NT subsystem surface directly.
- That is still thinner than introducing a new configuration or orchestration layer above NT.

## Unavoidable Custom Code

The custom code that remains necessary is:

- config translation
- secret resolution
- adapter/client factory dispatch
- filter construction
- strategy factory dispatch
- strategy implementations
- later custom venue adapters

Everything else should default to NT.

## What Must Not Be Built

Do not build:

- a runtime planner
- a component registry over NT
- a custom message bus wrapper
- a custom risk layer
- a custom portfolio tracker
- a custom order router
- a custom order state machine
- a generic platform schema for legs or subscriptions

If NT can already do it in Rust live, use NT directly.

## Future Compatibility

This design preserves:

- additional venues by adding more `[[data_clients]]` and `[[exec_clients]]`
- additional strategies by adding more `[[strategies]]`
- multi-instrument strategies inside strategy config
- cross-venue reads inside strategy logic
- future multi-venue execution inside strategy or exec algorithm logic
- rolling-market or expiring-instrument lifecycle logic inside strategy code
- later discovery logic without rewriting the platform schema
- later optional actors and exec algorithms without changing the core minimal shape

What may require later operational changes but not schema redesign:

- same-venue multi-account execution
- multi-node deployment
- cross-process coordination

## Open Caveats

- Some NT runtime surfaces are easy to deserialize directly; others still require small translation glue.
- Optional subsystem tuning should remain optional until actually needed.
- The implementation should strongly prefer NT defaults unless there is a clear operational reason to override them.

## Final Recommendation

Proceed with the thin-wrapper architecture.

`bolt-v2` should feel like:

- a small NT launcher
- plus strategies
- plus later missing adapters

It should not feel like a trading platform built on top of another trading platform.
