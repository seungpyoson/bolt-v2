# Reference Current Price Provider Ingestion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Chainlink and PRR reference-current-price ingestion end to end through the single NT custom-data path.

**Architecture:** Keep `reference_current_price` provider-agnostic. Provider clients own WebSocket connection, provider frame parsing, and `ReferencePriceUpdate` emission; the strategy only consumes normalized custom data and applies config-owned selection. Chainlink and PRR are first provider implementations, not strategy architecture.

**Tech Stack:** Rust 2024, NautilusTrader Rust APIs, `nautilus_network::websocket::WebSocketClient`, TOML config, AWS SSM via Rust SDK, Chainlink Data Streams WebSocket/HMAC/v3 reports, PRR WebSocket, GitHub CI.

---

## Guardrails Before Task 1

- Do not change implementation code until this plan and the design spec are approved by Claude, Gemini, and Grok.
- Do not run local `cargo test` unless the user changes the CI-only preference. Use CI for verification after pushing implementation commits.
- Do not print or log secrets.
- Keep every implementation commit small and TDD-shaped.
- Each code task starts by adding a failing test.

## File Responsibility Map

- `src/bolt_v3_reference_price.rs`: provider-agnostic `ReferencePriceUpdate`, provenance validation, custom-data conversion.
- `src/bolt_v3_providers/mod.rs`: provider metadata registry and reference-provider identifier rules.
- `src/bolt_v3_providers/chainlink/auth.rs`: Chainlink credential validation and HMAC headers for REST and WS request paths.
- `src/bolt_v3_providers/chainlink/report.rs`: Chainlink v3 report source parsing and full-report decode.
- `src/bolt_v3_providers/chainlink/strike_source.rs`: existing point-in-time `price_to_beat` source; keep strike-only behavior isolated.
- `src/bolt_v3_providers/chainlink_reference.rs`: Chainlink reference-current-price WebSocket data client.
- `src/bolt_v3_providers/polyresearch.rs`: PRR reference-current-price WebSocket data client.
- `src/bolt_v3_validate.rs`: config validation for reference-current-price sources and shared Chainlink feed catalog.
- `src/bolt_v3_reference_price_health.rs`: bounded health over the same strategy-free transport path.
- `src/strategies/binary_oracle_edge_taker/mod.rs`: strategy subscription and normalized custom-data consumption.
- `tests/bolt_v3_reference_price_config.rs`: reference-current-price config validation.
- `tests/bolt_v3_reference_price_runtime.rs`: normalized custom-data and selector behavior.
- `src/strategies/binary_oracle_edge_taker/tests/reference_price.rs`: strategy integration.
- `tests/bolt_v3_reference_provider_registration.rs`: provider registration and client block validation.
- `tests/bolt_v3_reference_current_price_ingestion.rs`: new integration tests for provider parser/client behavior when public APIs make this practical.

## Task 1: Close PRR Wire-Schema Evidence Gate

**Files:**
- Create or update: `docs/bolt-v3/research/prr-reference-current-price-wire-schema-2026-06-08.md`
- Modify: `docs/bolt-v3/research/reference-source-implementation-risk-check.md`

- [ ] **Step 1: Add the evidence note skeleton**

Create a research note with these exact sections:

```markdown
# PRR Reference Current Price Wire Schema - 2026-06-08

## Scope

This note records non-secret evidence for implementing PRR as a Bolt v3 reference-current-price provider.

## Secret Safety

- No secret values were printed.
- No credential-bearing endpoint URL was printed.
- No raw provider frame containing price values was printed unless explicitly approved.

## Evidence

## Runtime Contract

## Implementation Decision

## Stop Conditions
```

- [ ] **Step 2: Perform or attach authoritative evidence**

Use one approved source:

```text
Option A: bounded live probe using existing Rust/approved tooling, printing only field names, type map, symbol set, timestamp unit, subscribe requirement, heartbeat observation, and count summaries.
Option B: authoritative PRR documentation committed or linked in the note, with exact field names, timestamp units, auth placement, and subscribe semantics.
```

Expected evidence fields:

```text
endpoint source
credential placement
subscribe message requirement
heartbeat requirement
frame envelope shape
symbol field
price field
timestamp field and unit
supported symbols
malformed-frame behavior
```

- [ ] **Step 3: Stop if schema is not proven**

If neither evidence option proves the schema, stop implementation and report:

```text
PRR wire schema is not source-proven. Chainlink implementation can be planned, but PRR runtime parsing must not be implemented or claimed end to end.
```

- [ ] **Step 4: Update risk check**

Append a row to `reference-source-implementation-risk-check.md` recording PRR schema status as closed only if Step 2 proved it. Do not mark it closed from memory or the stale plan alone.

- [ ] **Step 5: Commit evidence only**

```bash
git add docs/bolt-v3/research/prr-reference-current-price-wire-schema-2026-06-08.md docs/bolt-v3/research/reference-source-implementation-risk-check.md
git commit -m "Document PRR reference price wire evidence"
```

## Task 2: Make Chainlink Feed Bindings Single-Sourced

**Files:**
- Modify: `src/bolt_v3_config.rs`
- Modify: `src/bolt_v3_providers/chainlink/mod.rs` or nearest existing Chainlink config module
- Modify: `src/bolt_v3_providers/chainlink/strike_source.rs`
- Modify: `src/bolt_v3_providers/chainlink_reference.rs`
- Modify: `src/bolt_v3_validate.rs`
- Modify: `config/root.toml`
- Modify: `tests/fixtures/bolt_v3/root.toml`
- Modify: `tests/config_parsing.rs`
- Modify: `tests/bolt_v3_reference_price_config.rs`

- [ ] **Step 1: Write failing config tests**

Add tests asserting:

```rust
#[test]
fn chainlink_reference_current_price_uses_shared_feed_catalog() {
    // Parse root config with one shared Chainlink feed catalog.
    // Validate a strategy whose reference_current_price.chainlink source uses
    // BTC-USD.CHAINLINK and whose resolution_data also uses BTC-USD.CHAINLINK.
    // Expected: no validation errors and both clients resolve the same feed id.
}

#[test]
fn chainlink_reference_current_price_rejects_missing_shared_feed_binding() {
    // Remove BTC-USD.CHAINLINK from the shared catalog.
    // Expected: validation error mentions reference_current_price source,
    // instrument_id, and missing Chainlink feed binding.
}

#[test]
fn root_rejects_duplicate_chainlink_feed_catalogs() {
    // Configure both old strike-local feed_bindings and new shared feed catalog.
    // Expected: validation error says Chainlink Data Streams feed bindings must
    // be single-sourced.
}
```

Expected CI result: these tests fail before implementation.

- [ ] **Step 2: Introduce one shared catalog type**

Add a TOML-owned shared Chainlink catalog type with fields equivalent to the current `feed_bindings` rows:

```rust
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkDataStreamsFeedCatalog {
    pub feed_bindings: Vec<ChainlinkDataStreamsFeedBinding>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkDataStreamsFeedBinding {
    pub feed_id: String,
    pub instrument_id: InstrumentId,
    pub report_schema_version: u64,
    pub report_decimal_scale: u64,
    pub price_precision: u8,
}
```

Use the repo's existing config style if these structs belong in an existing module rather than `bolt_v3_config.rs`.

- [ ] **Step 3: Move TOML values to one catalog**

Move the current `[[clients.chainlink_strike.data.feed_bindings]]` values into this exact root-owned shape:

```toml
[chainlink_data_streams]

[[chainlink_data_streams.feed_bindings]]
feed_id = "0x00037da000000000000000000000000000000000000000000000000000000001"
instrument_id = "BTC-USD.CHAINLINK"
report_schema_version = 3
report_decimal_scale = 18
price_precision = 8
```

Keep client sections referencing that catalog without duplicating feed rows:

```toml
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

Required invariant:

```text
Changing the Chainlink BTC feed id requires editing one TOML feed-binding row.
```

- [ ] **Step 4: Update strike source mapping**

Update `ChainlinkStrikeSourceConfig` construction so the strike source receives feed bindings from the shared catalog.

Preserve:

```text
strike source still emits IndexPriceUpdate only
strike source still requires window_open_unix_seconds
strike source still enforces validFrom == window open
```

- [ ] **Step 5: Update reference source mapping**

Update `ChainlinkReferencePriceClientConfig` construction so the reference source receives the same shared feed binding catalog.

- [ ] **Step 6: Commit**

```bash
git add src config tests
git commit -m "Single-source Chainlink Data Streams feed bindings"
```

## Task 3: Generalize Chainlink Auth And Report Decode For WebSocket Current Price

**Files:**
- Modify: `src/bolt_v3_providers/chainlink/auth.rs`
- Modify: `src/bolt_v3_providers/chainlink/report.rs`
- Modify: `src/bolt_v3_providers/chainlink/strike_source.rs`
- Modify: `src/bolt_v3_providers/chainlink_reference.rs`

- [ ] **Step 1: Write failing auth tests**

Add tests in `chainlink/auth.rs`:

```rust
#[test]
fn websocket_request_url_builds_signed_feed_ids_path() {
    let (url, path_with_query) = chainlink_data_streams_ws_request_url(
        "wss://ws.testnet-dataengine.chain.link",
        &["0x00037da000000000000000000000000000000000000000000000000000000001"],
    )
    .expect("valid ws request should build");

    assert_eq!(
        url,
        "wss://ws.testnet-dataengine.chain.link/api/v1/ws?feedIDs=0x00037da000000000000000000000000000000000000000000000000000000001"
    );
    assert_eq!(
        path_with_query,
        "/api/v1/ws?feedIDs=0x00037da000000000000000000000000000000000000000000000000000000001"
    );
}

#[test]
fn websocket_request_url_rejects_empty_feed_ids() {
    assert!(chainlink_data_streams_ws_request_url(
        "wss://ws.testnet-dataengine.chain.link",
        &[],
    )
    .is_err());
}
```

- [ ] **Step 2: Implement WS request builder**

Add a function equivalent to:

```rust
pub(crate) fn chainlink_data_streams_ws_request_url(
    websocket_endpoint: &str,
    feed_ids: &[String],
) -> Result<(String, String), BoltV3OperatorArtifactError> {
    // Validate wss endpoint, append /api/v1/ws, append feedIDs joined by comma,
    // return full URL and path_with_query for HMAC signing.
}
```

Use existing error types and validation style.

- [ ] **Step 3: Write failing current report decode tests**

Add tests proving the decoded v3 report exposes current-price fields:

```rust
#[test]
fn decoded_v3_report_exposes_price_bid_and_ask_for_reference_current_price() {
    let decoded = decode_chainlink_reference_price_report(&fixture_report_bytes(), &binding())
        .expect("fixture should decode");

    assert_eq!(decoded.feed_id, TEST_FEED_ID);
    assert_eq!(decoded.valid_from_timestamp_ms, TEST_VALID_FROM_MS);
    assert_eq!(decoded.observations_timestamp_ms, TEST_OBSERVATIONS_MS);
    assert_eq!(decoded.price, TEST_PRICE);
    assert_eq!(decoded.bid, TEST_BID);
    assert_eq!(decoded.ask, TEST_ASK);
}
```

- [ ] **Step 4: Generalize report structs**

Keep strike behavior stable while exposing a current-price decode struct:

```rust
pub(crate) struct DecodedChainlinkV3Report {
    pub(crate) feed_id: String,
    pub(crate) valid_from_timestamp_ms: u64,
    pub(crate) observations_timestamp_ms: u64,
    pub(crate) price: f64,
    pub(crate) bid: Option<f64>,
    pub(crate) ask: Option<f64>,
}
```

`DecodedPriceToBeatReport` may become a wrapper or alias that preserves existing strike tests and call sites.

- [ ] **Step 5: Commit**

```bash
git add src/bolt_v3_providers/chainlink
git commit -m "Prepare Chainlink reports for reference current price"
```

## Task 4: Implement Chainlink Reference DataClient Ingestion

**Files:**
- Modify: `src/bolt_v3_providers/chainlink_reference.rs`
- Modify: `src/bolt_v3_reference_price.rs` if provenance validation needs new non-secret keys
- Modify: `tests/bolt_v3_reference_provider_registration.rs`
- Add or modify focused provider tests in the nearest test module

- [ ] **Step 1: Write failing parser test**

Add a pure test:

```rust
#[test]
fn chainlink_ws_report_maps_to_reference_price_update() {
    let update = chainlink_reference_update_from_report(
        &fixture_subscription("BTC", "chainlink_primary", "BTC-USD.CHAINLINK"),
        &fixture_decoded_report(TEST_FEED_ID, 66_300.25, Some(66_299.0), Some(66_301.0)),
        TEST_RECEIVED_TS_MS,
    )
    .expect("valid report should map");

    assert_eq!(update.asset(), "BTC");
    assert_eq!(update.source_id(), "chainlink_primary");
    assert_eq!(update.provider(), "chainlink_ws");
    assert_eq!(update.provider_instrument(), "BTC-USD.CHAINLINK");
    assert_eq!(update.price(), 66_300.25);
    assert_eq!(update.observed_ts_ms(), TEST_OBSERVATIONS_TS_MS);
}
```

- [ ] **Step 2: Write failing subscribe test**

Add a test that constructs the Chainlink client with fake transport hooks or direct handler access:

```rust
#[test]
fn chainlink_reference_subscribe_registers_source_by_data_type_and_feed() {
    let mut client = fixture_chainlink_reference_client();
    client.subscribe(fixture_subscribe_custom_data(
        "BTC",
        "chainlink_primary",
        "chainlink_ws",
        "BTC-USD.CHAINLINK",
    ))
    .expect("subscription should register");

    assert!(client.has_active_reference_subscription("chainlink_primary"));
}
```

- [ ] **Step 3: Add data sender and active subscription state**

The client struct should contain:

```rust
struct ChainlinkReferencePriceClient {
    client_id: ClientId,
    config: ChainlinkReferencePriceClientConfig,
    connected: bool,
    data_sender: tokio::sync::mpsc::UnboundedSender<DataEvent>,
    ws_client: Option<nautilus_network::websocket::WebSocketClient>,
    active_subscriptions: Arc<Mutex<BTreeMap<String, ChainlinkReferenceSubscription>>>,
}
```

Use existing project concurrency patterns if `Mutex` is not the local norm.

- [ ] **Step 4: Implement real connect/disconnect**

`connect()` must:

```text
build feedIDs from configured active subscriptions or connect lazily when first subscription arrives
build Chainlink WS URL
build HMAC headers using the path_with_query
open WebSocketClient handler mode
set connected true only after successful connect
```

`disconnect()` must:

```text
disconnect WebSocketClient if present
clear connected state
preserve config
```

- [ ] **Step 5: Implement subscribe/unsubscribe**

`subscribe(SubscribeCustomData)` must validate and record subscriptions. If the provider requires reconnect to alter `feedIDs`, use one clear policy:

```text
Either connect after subscriptions are known, or reconnect with the full configured feedID set when subscriptions change.
```

Do not keep two policies.

- [ ] **Step 6: Implement message handler emission**

The handler must:

```text
parse text message JSON
decode Chainlink v3 report
match decoded feed id to an active subscription
build ReferencePriceUpdate
send DataEvent::Data(Data::Custom(update.to_custom_data()))
log and drop malformed/unmatched messages
```

- [ ] **Step 7: Commit**

```bash
git add src tests
git commit -m "Emit Chainlink reference current price updates"
```

## Task 5: Implement PRR Reference DataClient Ingestion

**Files:**
- Modify: `src/bolt_v3_providers/polyresearch.rs`
- Modify: `src/bolt_v3_reference_price.rs` if provenance validation needs new non-secret keys
- Modify: `tests/bolt_v3_polyresearch_auth.rs`
- Add or modify focused provider tests in the nearest test module

- [ ] **Step 1: Verify Task 1 is closed**

Before editing code, confirm the PRR wire evidence note has `Implementation Decision` stating parsing is approved and source-proven.

- [ ] **Step 2: Write failing parser tests**

Use the source-proven frame shape. If Task 1 confirms `{"symbol":"BTC/USD","price":66300.25,"ts":1774672588000}`, add:

```rust
#[test]
fn prr_price_frame_maps_to_reference_price_update() {
    let update = prr_reference_update_from_frame(
        &fixture_subscription("BTC", "polyresearch_primary", "BTC"),
        r#"{"symbol":"BTC/USD","price":66300.25,"ts":1774672588000}"#,
        TEST_RECEIVED_TS_MS,
    )
    .expect("valid PRR frame should map");

    assert_eq!(update.asset(), "BTC");
    assert_eq!(update.source_id(), "polyresearch_primary");
    assert_eq!(update.provider(), "polyresearch_ws");
    assert_eq!(update.provider_instrument(), "BTC");
    assert_eq!(update.price(), 66_300.25);
    assert_eq!(update.observed_ts_ms(), 1_774_672_588_000);
}

#[test]
fn prr_price_frame_rejects_wrong_symbol() {
    assert!(prr_reference_update_from_frame(
        &fixture_subscription("BTC", "polyresearch_primary", "BTC"),
        r#"{"symbol":"ETH/USD","price":3000.0,"ts":1774672588000}"#,
        TEST_RECEIVED_TS_MS,
    )
    .is_err());
}
```

If Task 1 proves a different frame shape, use that exact shape instead.

- [ ] **Step 3: Add data sender and active subscription state**

The client struct should contain:

```rust
struct PolyResearchReferencePriceClient {
    client_id: ClientId,
    config: PolyResearchReferencePriceClientConfig,
    connected: bool,
    data_sender: tokio::sync::mpsc::UnboundedSender<DataEvent>,
    ws_client: Option<nautilus_network::websocket::WebSocketClient>,
    active_subscriptions: Arc<Mutex<BTreeMap<String, PolyResearchReferenceSubscription>>>,
}
```

- [ ] **Step 4: Implement real connect/disconnect**

`connect()` must build the credentialed URL using `polyresearch_websocket_url`, open WebSocketClient handler mode, and set connected true only after successful connect.

`disconnect()` must close the WebSocket client and clear connected state.

- [ ] **Step 5: Implement subscribe/unsubscribe**

`subscribe(SubscribeCustomData)` must validate `DataType`, source id, provider key, asset, and configured `symbol`.

If Task 1 proves PRR requires a subscribe message, send that exact provider message in `subscribe`. If Task 1 proves no subscribe message is required, do not invent one.

- [ ] **Step 6: Implement message handler emission**

The handler must:

```text
parse text message JSON
match provider symbol to active subscription
build ReferencePriceUpdate
send DataEvent::Data(Data::Custom(update.to_custom_data()))
log and drop malformed/unmatched messages
```

- [ ] **Step 7: Commit**

```bash
git add src tests
git commit -m "Emit PRR reference current price updates"
```

## Task 6: Tighten Provider Metadata And Config Demonstration

**Files:**
- Modify: `src/bolt_v3_providers/mod.rs`
- Modify: `src/bolt_v3_validate.rs`
- Modify: `config/root.toml`
- Modify: `config/strategies/binary_oracle_btc.toml`
- Modify: `config/strategies/binary_oracle_eth.toml`
- Modify: `config/strategies/binary_oracle_sol.toml`
- Modify: `config/strategies/binary_oracle_xrp.toml`
- Modify if supported: `config/strategies/binary_oracle_bnb.toml`
- Modify if supported: `config/strategies/binary_oracle_doge.toml`
- Modify: `tests/bolt_v3_reference_price_config.rs`
- Modify: `tests/config_parsing.rs`

- [ ] **Step 1: Write failing metadata-driven validation test**

Add a test proving validation does not hardcode Chainlink/PRR-specific strategy logic:

```rust
#[test]
fn reference_current_price_identifier_errors_are_provider_metadata_driven() {
    let messages = validate_reference_current_price(/* PRR source with instrument_id */);
    assert!(messages.iter().any(|message| {
        message.contains("reference_current_price.source.polyresearch_primary")
            && message.contains("symbol")
            && message.contains("polyresearch_ws")
    }));
}
```

- [ ] **Step 2: Add PRR active-source fixture**

Add a fixture or test TOML where PRR is the only source for BTC:

```toml
[reference_current_price]
asset = "BTC"
sources = ["polyresearch_primary"]
min_valid_sources = 1
selection_policy = "first_valid_per_interval"
max_source_age_ms = 2000
max_source_drift_bps = 25
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source.polyresearch_primary]
provider = "polyresearch_ws"
client_id = "polyresearch_reference"
symbol = "BTC"
required = true
```

Expected: validation passes for supported PRR assets.

- [ ] **Step 3: Add shipped config support**

Ensure root config has a valid `polyresearch_reference` data client if PRR shipped strategy sources are enabled.

For assets supported by provider metadata, either:

```text
Add PRR as optional backup source in shipped configs
```

or:

```text
Add an explicit shipped PRR-active strategy fixture used by tests and operators.
```

Do not make unsupported assets require PRR.

- [ ] **Step 4: Commit**

```bash
git add src config tests
git commit -m "Demonstrate provider-agnostic reference current price config"
```

## Task 7: Upgrade Health To Observe Provider Updates Through The Same Path

**Files:**
- Modify: `src/bolt_v3_reference_price_health.rs`
- Modify: `src/main.rs` only if CLI output fields change
- Modify: `tests/cli.rs`
- Modify focused health tests

- [ ] **Step 1: Write failing health test**

Add a test proving health expects provider update observation:

```rust
#[test]
fn reference_current_price_health_reports_observed_updates_per_source() {
    let result = run_health_with_fake_reference_provider_update("chainlink_primary");

    assert_eq!(result.source("chainlink_primary").connect, HealthStatus::Ok);
    assert_eq!(result.source("chainlink_primary").subscribe, HealthStatus::Ok);
    assert_eq!(result.source("chainlink_primary").first_update, HealthStatus::Ok);
}
```

- [ ] **Step 2: Preserve no-order invariant**

Add or preserve a test:

```rust
#[test]
fn reference_current_price_health_does_not_register_strategies_or_enter_order_path() {
    let health_run = prepare_reference_current_price_health_run(&loaded_config()).unwrap();
    assert!(health_run.live_node.trader().strategies().is_empty());
    assert!(!health_run.enters_runner_loop());
}
```

Use actual accessible APIs; if `enters_runner_loop()` does not exist, assert the concrete no-runner/no-order condition available in the current code.

- [ ] **Step 3: Implement bounded observe**

Health should:

```text
build strategy-free live node
connect registered transport clients
issue provider subscribe commands for configured reference sources
wait bounded time for Data::Custom ReferencePriceUpdate events
record per-source observed/not_observed
unsubscribe
disconnect
return structured status
```

- [ ] **Step 4: Commit**

```bash
git add src tests
git commit -m "Observe reference current price updates in health"
```

## Task 8: End-To-End Strategy Integration Tests

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/tests/reference_price.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs`
- Modify: `tests/bolt_v3_reference_price_runtime.rs`

- [ ] **Step 1: Add Chainlink provider emission integration test**

Add a test that starts from a provider-mapped `ReferencePriceUpdate` and proves entry pricing uses it:

```rust
#[test]
fn chainlink_provider_update_feeds_entry_pricing_spot_without_binding_price_to_beat() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.reference_current_price = Some(chainlink_reference_price_config());
    let _cache = register_test_strategy(&mut strategy);

    DataActor::on_data(&mut strategy, &chainlink_provider_custom_update()).unwrap();

    let inputs = strategy.current_entry_pricing_inputs_at(TEST_NOW_MS).unwrap();
    assert_eq!(inputs.spot_price, TEST_CHAINLINK_PRICE);
    assert_eq!(strategy.active.price_to_beat, Some(TEST_STRIKE_PRICE));
}
```

- [ ] **Step 2: Add PRR provider emission integration test**

Add equivalent PRR test:

```rust
#[test]
fn prr_provider_update_feeds_entry_pricing_spot_without_binding_price_to_beat() {
    let mut strategy = ready_to_trade_strategy();
    strategy.config.reference_current_price = Some(prr_reference_price_config());
    let _cache = register_test_strategy(&mut strategy);

    DataActor::on_data(&mut strategy, &prr_provider_custom_update()).unwrap();

    let inputs = strategy.current_entry_pricing_inputs_at(TEST_NOW_MS).unwrap();
    assert_eq!(inputs.spot_price, TEST_PRR_PRICE);
    assert_eq!(strategy.active.price_to_beat, Some(TEST_STRIKE_PRICE));
}
```

- [ ] **Step 3: Add explicit no-price-to-beat tests**

Add tests proving current-price custom data does not set `price_to_beat` when strike is absent:

```rust
#[test]
fn reference_current_price_update_never_sets_missing_price_to_beat() {
    let mut strategy = ready_to_trade_strategy_without_strike();
    strategy.config.reference_current_price = Some(prr_reference_price_config());

    DataActor::on_data(&mut strategy, &prr_provider_custom_update()).unwrap();

    assert!(strategy.active.reference_current_price.is_some());
    assert!(strategy.active.price_to_beat.is_none());
}
```

- [ ] **Step 4: Commit**

```bash
git add src/strategies tests
git commit -m "Cover provider reference updates in strategy pricing"
```

## Task 9: CI And External Review Gate

**Files:**
- No code files required unless CI or reviews find issues.

- [ ] **Step 1: Push implementation branch**

```bash
git push
```

- [ ] **Step 2: Use CI instead of local cargo tests**

Wait for exact-head GitHub CI:

```text
actionlint success
Backtester CI success
CI success, including fmt-check, clippy, source-fence, all nextest shards, test, gate
```

- [ ] **Step 3: Request external reviews**

Ask Claude, Gemini, and Grok to review the exact PR diff against this spec and plan.

Required review prompt:

```text
Review PR #606 against:
- docs/superpowers/specs/2026-06-08-reference-current-price-provider-ingestion-design.md
- docs/superpowers/plans/2026-06-08-reference-current-price-provider-ingestion.md

Goal: Chainlink and PRR must be usable end to end for reference_current_price through the single NT DataClient/SubscribeCustomData/Data::Custom/ReferencePriceUpdate path into trading decisions.

Block on:
- dual paths or side channels,
- fake provider connection state,
- provider-specific strategy selection logic,
- Chainlink feed-id duplication,
- PRR parsing without source-proven schema,
- PRR or Chainlink current price binding price_to_beat,
- non-SSM credential source,
- missing CI or live verification evidence.
```

- [ ] **Step 4: Address findings with TDD**

Every substantive finding must become:

```text
RED test
minimal fix
CI verification
reply to reviewer
resolve thread if applicable
```

- [ ] **Step 5: Commit**

```bash
git add .
git commit -m "Address reference provider review findings"
```

Only commit if files changed.

## Task 10: Live Verification

**Files:**
- No code files expected.

- [ ] **Step 1: Confirm exact-head CI green**

Do not start live verification unless exact-head CI is green.

- [ ] **Step 2: Confirm target and approval**

Use the operator-approved target only. Do not mutate AWS SSM. Do not print secrets.

- [ ] **Step 3: Run bounded live verification**

Required evidence:

```text
Chainlink reference provider connects
Chainlink reference provider subscribes
Chainlink ReferencePriceUpdate observed
PRR reference provider connects
PRR reference provider subscribes
PRR ReferencePriceUpdate observed
selected active.reference_current_price updates trading spot
price_to_beat remains sourced from resolution_data
no order path entered during health verification
service restored to inactive/disabled if that was the precondition
```

- [ ] **Step 4: Update PR evidence**

Update PR #606 body/comments with exact-head CI and live evidence. If live verification cannot observe a provider update, record the concrete blocker and do not claim end-to-end completion.
