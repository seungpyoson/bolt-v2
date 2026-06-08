# Reference Current Price Provider Ingestion Design

## Status

Approved direction from the user on 2026-06-08: proceed with a provider-agnostic `reference_current_price` design and get external model approval before implementation code changes.

This document is a design/spec artifact only. It intentionally does not change runtime code.

## Goal

Make `reference_current_price` usable end to end for live trading through the normal NautilusTrader data path. Chainlink Data Streams WebSocket and PRR are the first concrete providers, but the architecture must remain provider-agnostic.

The completed implementation must prove this chain:

1. TOML selects one or more `reference_current_price` sources.
2. The selected source client is registered as an NT data client.
3. The provider client really connects to the provider transport.
4. The strategy's `subscribe_data` request reaches the provider through `DataClient::subscribe(SubscribeCustomData)`.
5. Provider frames are parsed and normalized into `ReferencePriceUpdate`.
6. The provider emits `DataEvent::Data(Data::Custom(update.to_custom_data()))`.
7. NT routes that custom data to the strategy `on_data` handler.
8. The strategy selector accepts a source according to config.
9. The selected source updates `active.reference_current_price` and `TakerPricingState`.
10. Entry pricing uses that selected current price as the trading spot input.

## Non-Negotiable Rules

- One runtime data path: NT `DataClient` plus `SubscribeCustomData` plus `Data::Custom`.
- No sidecar, materializer, isolated probe node, msgbus bypass, or strategy-local provider parser.
- No fake provider connect state. `connect()` must represent actual provider transport readiness.
- No retired `reference_data`, `reference_venue`, or `reference_instrument_id` path.
- No provider-specific source selection in the strategy. Strategy code consumes provider-agnostic `ReferencePriceUpdate`.
- No PRR or Chainlink reference update may ever bind `price_to_beat`.
- All credentials resolve through existing Rust SSM secret resolution. No environment, local file, 1Password, AWS CLI subprocess, Python, or fallback secret source.
- Provider-specific protocol constants stay in provider modules or audited provider-owned protocol modules.
- Runtime values remain TOML-owned. Feed IDs, endpoints, timeouts, thresholds, source order, and required/optional policy are not hardcoded in strategy logic.

## Current-State Evidence

Current PR head inspected: `ad98423077b60518b452073d6344b5e1f3db971e`.

The current branch already has the provider-agnostic internal strategy contract:

- `src/bolt_v3_reference_price.rs` defines `ReferencePriceUpdate`, `data_type_for`, `to_custom_data`, and `from_custom_data`.
- `src/strategies/binary_oracle_edge_taker/mod.rs` builds `ReferencePriceUpdate` custom-data subscriptions from `[reference_current_price]`.
- Strategy `on_data` handles `ReferencePriceUpdate`, validates configured source/provider/provider instrument, selects a quote, and updates `TakerPricingState`.
- `src/bolt_v3_taker_pricing.rs` uses the selected current reference price as `fast_spot`, which feeds entry pricing.
- NT's `DataClient` trait supports `subscribe(SubscribeCustomData)`, and NT's data engine publishes `CustomData` by `DataType`.

The current branch is incomplete against the original goal:

- `src/bolt_v3_providers/chainlink_reference.rs` only toggles `connected`; it does not open Chainlink WebSocket transport, override `subscribe`, parse provider reports, or emit `Data::Custom`.
- `src/bolt_v3_providers/polyresearch.rs` only toggles `connected`; it does not open PRR WebSocket transport, override `subscribe`, parse provider frames, or emit `Data::Custom`.
- PR #606 currently says low-level provider ingestion is out of scope. That is wrong for the original goal and must be corrected when this spec is implemented.

## Provider-Agnostic Contract

`reference_current_price` remains strategy-scoped and ordered by source id:

```toml
[reference_current_price]
asset = "BTC"
sources = ["chainlink_primary", "polyresearch_backup"]
min_valid_sources = 1
selection_policy = "first_valid_per_interval"
max_source_age_ms = 2000
max_source_drift_bps = 25
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source.chainlink_primary]
provider = "chainlink_ws"
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"
required = true

[reference_current_price.source.polyresearch_backup]
provider = "polyresearch_ws"
client_id = "polyresearch_reference"
symbol = "BTC"
required = false
```

The strategy must only know:

- source id,
- provider key,
- client id,
- provider identifier params,
- source order and selection policy.

Provider-specific details are resolved in provider metadata and provider clients. The existing provider metadata registry should be extended so validation messages and subscription parameter checks are metadata-driven rather than hardcoded around Chainlink and PRR. Chainlink and PRR can be first-class metadata entries, but they must not become the strategy architecture.

## Chainlink Design

Chainlink reference current price uses Chainlink Data Streams WebSocket, not the existing REST strike source.

Authoritative protocol evidence:

- Official Chainlink Data Streams WebSocket docs list the testnet/mainnet WebSocket domains.
- Official Chainlink WebSocket docs use endpoint `/api/v1/ws` with comma-separated `feedIDs`.
- Official Chainlink authentication docs require `Authorization`, `X-Authorization-Timestamp`, and `X-Authorization-Signature-SHA256` headers.
- Official Chainlink v3 report schema includes `feedId`, `validFromTimestamp`, `observationsTimestamp`, `price`, `bid`, and `ask`.

Existing repo protocol code to reuse:

- `src/bolt_v3_providers/chainlink/auth.rs` owns credential validation and HMAC header construction.
- `src/bolt_v3_providers/chainlink/report.rs` owns v3 report validation and decode logic for `fullReport`.
- `src/bolt_v3_providers/chainlink/strike_source.rs` proves the correct NT pattern for emitting provider data through `get_data_event_sender()`.

Required implementation shape:

- Add a Chainlink WebSocket request builder for `wss://.../api/v1/ws?feedIDs=...`.
- Reuse or generalize Chainlink HMAC header code without coupling current-price WebSocket ingestion to `price_to_beat`.
- Reuse or generalize v3 report decode so current price can parse `price`, `bid`, and `ask`.
- For current reference price, do not apply the strike-only `validFrom == window_open` check. That check remains exclusive to `price_to_beat`.
- Map the decoded Chainlink report to `ReferencePriceUpdate`:
  - `asset`: strategy/source asset from subscription params,
  - `source_id`: source key from subscription params,
  - `provider`: `chainlink_ws`,
  - `provider_instrument`: configured `instrument_id`,
  - `price`: decoded v3 `price`,
  - `bid`: decoded v3 `bid`,
  - `ask`: decoded v3 `ask`,
  - `observed_ts_ms`: decoded `observationsTimestamp` converted to milliseconds,
  - `received_ts_ms`: local receive timestamp,
  - `provenance`: non-secret fields such as feed id, valid-from timestamp, observations timestamp, and schema version.

## Chainlink Feed Catalog

The implementation must not duplicate Chainlink feed ids across current-price and strike clients.

Current root config keeps Chainlink `feed_bindings` under `clients.chainlink_strike.data`. Current `clients.chainlink_reference.data` has no feed bindings. Adding a second feed-binding list under `chainlink_reference` would create two edit locations for one Chainlink feed change.

Required design:

- Move Chainlink Data Streams feed bindings to one shared TOML-owned catalog:

```toml
[chainlink_data_streams]

[[chainlink_data_streams.feed_bindings]]
feed_id = "0x00037da000000000000000000000000000000000000000000000000000000001"
instrument_id = "BTC-USD.CHAINLINK"
report_schema_version = 3
report_decimal_scale = 18
price_precision = 8

[clients.chainlink_strike.data]
rest_base_url = "https://api.testnet-dataengine.chain.link"
report_endpoint_path = "/api/v1/reports"
http_timeout_secs = 10
feed_catalog = "chainlink_data_streams"

[clients.chainlink_reference.data]
websocket_endpoint = "wss://ws.testnet-dataengine.chain.link"
transport_backend = "sockudo"
heartbeat_secs = 5
heartbeat_message = "ping"
reconnect_timeout_ms = 5000
reconnect_delay_initial_ms = 250
reconnect_delay_max_ms = 5000
reconnect_backoff_factor = 1.5
reconnect_jitter_ms = 100
idle_timeout_ms = 10000
feed_catalog = "chainlink_data_streams"
```

- Both `chainlink_strike` and `chainlink_reference` must resolve `instrument_id -> feed_id` from `chainlink_data_streams.feed_bindings`.
- Existing `resolution_data.instrument_id` and `reference_current_price.source.*.instrument_id` continue selecting the logical NT/provider instrument.
- Validation must fail if a configured Chainlink current-price instrument has no feed binding in the single catalog.
- Validation must fail if old client-local `feed_bindings` coexist with the shared catalog.
- Changing the BTC Chainlink feed id must require editing only one `[[chainlink_data_streams.feed_bindings]]` row.

## PRR Design

PRR is a provider implementation, not a generic architecture constraint.

Current repo evidence:

- `src/bolt_v3_providers/polyresearch.rs` owns PRR provider registration and credential redaction.
- `polyresearch_websocket_url` appends `key` exactly once and rejects endpoints already containing `key` or legacy credential query `apiKey`.
- `POLYRESEARCH_REFERENCE_PRICE_SUPPORTED_ASSETS` currently allows BTC, ETH, SOL, XRP.
- Prior local plan evidence records a secret-safe live PRR probe showing JSON text frames with key set `price,symbol,ts`, symbols `BTC/USD`, `ETH/USD`, `SOL/USD`, `XRP/USD`, no initial subscribe message, and no application heartbeat required over the probe window.

Evidence gap:

- The raw PRR probe artifact is not present in this worktree.
- Public docs for the exact PolyResearch PRR wire schema were not found in the current inspection.

Required pre-code confidence gate:

- Before PRR parsing/runtime code changes, re-prove the PRR wire schema with a bounded secret-safe live probe or add authoritative PRR documentation to the repo.
- The proof must not print secret values, raw credential-bearing URLs, or raw frames containing prices if that is considered sensitive.
- The proof must establish endpoint source, credential placement, whether a subscribe message is required, heartbeat expectations, symbol format, timestamp unit, and exact price field shape.

Required implementation shape after the gate passes:

- `connect()` opens the configured PRR WebSocket transport with the API key attached exactly once.
- `subscribe(SubscribeCustomData)` records desired source subscriptions and, only if the provider requires it, sends provider-specific subscribe messages.
- Incoming PRR frames are parsed by provider-owned pure parser code.
- The parser maps provider symbols such as `BTC/USD` or the gate-proven equivalent to the configured `reference_current_price.asset`.
- Parsed frames map to `ReferencePriceUpdate`:
  - `asset`: configured asset,
  - `source_id`: source key,
  - `provider`: `polyresearch_ws`,
  - `provider_instrument`: configured symbol,
  - `price`: provider price field,
  - `bid`: if provider supplies one, otherwise `None`,
  - `ask`: if provider supplies one, otherwise `None`,
  - `observed_ts_ms`: provider timestamp normalized to milliseconds,
  - `received_ts_ms`: local receive timestamp,
  - `provenance`: non-secret provider frame metadata.

## NT DataClient Runtime Shape

Both provider clients must use the existing NT runtime pattern:

- Store `client_id`, resolved config, `connected` state, `data_sender`, WebSocket client handle, and active subscription map.
- `connect()` opens transport and installs a message handler.
- `disconnect()` closes transport and clears connected state.
- `subscribe(SubscribeCustomData)` validates:
  - requested `DataType` is `BoltV3ReferencePriceUpdate`,
  - metadata and params match asset/source/provider,
  - required provider identifier is present,
  - provider identifier has a feed or symbol mapping,
  - subscription does not conflict with an existing source mapping.
- `unsubscribe(&UnsubscribeCustomData)` removes the matching active subscription and, if applicable, sends provider unsubscribe.
- Message handler emits only validated updates for active subscriptions.
- Malformed frames fail closed with logs/status counters but do not panic.
- Provider clients never submit orders and never mutate strategy state directly.

## Strategy And Pricing Behavior

The existing strategy behavior should remain the target:

- Strategy subscribes to all enabled `reference_current_price` sources on start and unsubscribes on stop.
- Strategy accepts only `ReferencePriceUpdate` whose source id, provider key, asset, and provider instrument match config.
- The selector chooses according to TOML policy.
- Selected current price updates:
  - `active.reference_current_price`,
  - `active.reference_current_price_source_id`,
  - `active.reference_current_price_ts_ms`,
  - `TakerPricingState.last_reference_current_price`,
  - `TakerPricingState.fast_spot`.
- Entry pricing reads the selected current price as spot.
- Out-of-order, stale, malformed, wrong-source, wrong-provider, and wrong-provider-instrument updates fail closed.

## Health And Live Verification

The health path must be a diagnostic over the same provider transport path, not a second architecture.

Required shape:

- Keep one strategy-free transport health path, based on the existing strategy-free LiveNode build.
- Do not reintroduce isolated data-client probe nodes.
- Health may connect, subscribe, observe bounded `ReferencePriceUpdate` events, unsubscribe, and disconnect.
- Health must not enter NT strategy runner/order loops.
- Health must report per-source connect, subscribe, first-update observation, and stop status.
- Live verification must run only after exact-head CI is green and operator approval is present.

## Shipped Config Requirement

The repository must demonstrate that PRR is usable, not just registered.

Required config outcomes:

- Shipped BTC/ETH/SOL/XRP strategy config should have a valid way to select Chainlink and a valid way to select PRR without code changes.
- BNB/DOGE must not configure PRR as required while provider metadata says PRR does not support them.
- A test fixture must prove PRR can be the selected active source for a supported asset.
- A test fixture must prove Chainlink can be the selected active source for a supported asset.
- A test fixture must prove an unknown future provider can be rejected by metadata validation without changing strategy logic.

## Testing Requirements

Use TDD for implementation. The first implementation step for each behavior is a failing test.

Required test groups:

- Config validation:
  - provider metadata drives identifier requirements,
  - Chainlink current-price source requires a configured feed binding,
  - PRR source supports configured assets only,
  - provider/client venue mismatch fails,
  - one Chainlink feed catalog feeds both strike and reference.
- Pure parsers:
  - Chainlink WS report frame maps to `ReferencePriceUpdate`,
  - Chainlink wrong feed id fails closed,
  - Chainlink malformed `fullReport` fails closed,
  - PRR frame maps to `ReferencePriceUpdate`,
  - PRR wrong symbol or bad timestamp fails closed.
- Provider clients:
  - `subscribe(SubscribeCustomData)` registers active subscriptions,
  - provider frame emits `Data::Custom`,
  - unmatched frame does not emit,
  - unsubscribe stops emission for that source,
  - connect/disconnect reflect real transport lifecycle in tests through an injected or local fake transport.
- Strategy integration:
  - Chainlink emitted update feeds entry pricing,
  - PRR emitted update feeds entry pricing,
  - PRR update cannot bind `price_to_beat`,
  - Chainlink current update cannot bind `price_to_beat`,
  - stale/wrong-provider/wrong-instrument updates fail closed.
- Health:
  - bounded health subscribes and observes an update through the same provider path,
  - health does not register strategies or enter order path,
  - health reports missing update separately from failed connect.
- Verification:
  - CI, source fence, clippy, and formatting must pass on exact PR head.
  - Live verification must prove at least one Chainlink update and one PRR update can reach the selected reference-current-price path before claiming end-to-end completion.

## Completion Criteria

The PR is not complete until all of the following are true:

- Chainlink WebSocket provider can produce live `ReferencePriceUpdate` from provider frames.
- PRR provider can produce live `ReferencePriceUpdate` from provider frames.
- Both providers route through NT `DataClient::subscribe(SubscribeCustomData)` and `Data::Custom`.
- Strategy selection and pricing consume those updates.
- `price_to_beat` remains isolated to `resolution_data` and Chainlink strike source.
- PRR schema evidence gate is closed with source-grounded evidence.
- Config has no duplicate feed-id lifecycle path.
- Exact-head CI is green.
- External reviews from Claude, Gemini, and Grok approve the spec/plan or all findings are addressed.
- Live verification on the approved target observes both provider paths or records a concrete provider-side blocker without claiming completion.
