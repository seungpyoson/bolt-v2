# Bolt V3 Reference Runtime Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Bolt v3 live reference-price path explicit, eu-west-2 sourced, fail-loud, and separate from boundary `price_to_beat` and retired evidence gates.

**Architecture:** Do this as separate PR-sized slices. First finish the current #581 hardening branch so eu-west-2 config and tests agree. Then remove the dead evidence-gate apparatus from a fresh branch for #579. Then build the proper `reference_price` provider architecture so continuous reference quotes are TOML-selected and never inferred from `decision_reference` or Chainlink boundary strike config.

**Tech Stack:** Rust 2024, NautilusTrader Rust APIs, TOML config, AWS SSM through Rust SDK only, Chainlink Data Streams REST/WS, PRR WS, `cargo test`, `cargo clippy`, `just source-fence`.

---

## Current Evidence

- Branch: `feat/581-reference-source-generic`, ahead of `origin/main` by 5 commits.
- `config/root.toml` and `tests/fixtures/bolt_v3/root.toml` use `[aws].region = "eu-west-2"` and `/bolt/polymarket/*`.
- Name-only SSM inventory found these eu-west-2 paths: `/bolt/polymarket/api-key`, `/bolt/polymarket/api-passphrase`, `/bolt/polymarket/api-secret`, `/bolt/polymarket/private-key`, `/bolt/testnet/chainlink/api-key`, `/bolt/testnet/chainlink/api-secret`, `/bolt/polyresearch/api-key`, `/bolt/polyresearch/websocket-endpoint`.
- `cargo test --locked --test bolt_v3_strategy_registration binary_oracle_runtime_mapping -- --nocapture` passes.
- `cargo test --locked --test config_parsing shipped_polymarket_secrets_use_eu_west_2_registry_paths -- --nocapture` passes.
- `cargo test --locked --test bolt_v3_client_registration -- --nocapture` passes after fake SSM resolvers and stale assertions were updated to `/bolt/polymarket/*`.
- Current Chainlink provider is a point-in-time strike source only, documented in `src/bolt_v3_providers/chainlink.rs`; it is not a continuous reference-price stream.
- Shipped strategies still have empty `[reference_data]` and still declare `target.gate_subscriptions.decision_reference`.
- Nautilus Rust supports typed custom streams: `DataActor::subscribe_data(DataType, Some(ClientId), params)` sends `SubscribeCustomData`, provider `DataClient::subscribe(SubscribeCustomData)` receives it, providers can emit `Data::Custom(CustomData)`, and strategies can override `on_data(&CustomData)`.
- Nautilus Rust already includes a reusable `nautilus_network::websocket::WebSocketClient` with custom handshake headers, reconnect, heartbeat, and transport-backend support. Do not add a separate WebSocket dependency for PRR or Chainlink reference streaming unless this API proves insufficient in implementation.
- Adjacent PRR/Chainlink probe docs prove PRR message schema and limits: text frames with `symbol`, `price`, `ts`; supported symbols are BTC, ETH, SOL, XRP; PRR cannot supply boundary `price_to_beat`.
- The current `docs/bolt-v3/research/reference-source-implementation-risk-check.md` closes SSM credential-name readiness only. It does not close PRR credential placement, subscribe message, or heartbeat semantics for a runtime WebSocket provider.
- `op item list --vault Development --format=json` and `op account list --format=json` failed locally because the 1Password CLI could not connect to the desktop app, so the `prr-price-feed-setup.md` attachment has not been verified in this shell.
- A secret-safe live PRR probe on 2026-06-06 fetched SSM values into process variables without printing them. It proved the pre-cleanup endpoint value was a `wss:` URL with query key `apiKey`; a manual WebSocket upgrade using the endpoint as-is returned HTTP `101`; adding `Authorization`, `Authorization: Bearer`, `X-API-Key`, or `x-api-key` headers was unnecessary because the endpoint already carried `apiKey`.
- The same probe proved the pre-cleanup endpoint query `apiKey` value was length `36` and equaled the separate `/bolt/polyresearch/api-key` value, so the old SSM representation duplicated the same credential in two parameters.
- PRR SSM cleanup completed on 2026-06-06 in eu-west-2: `/bolt/polyresearch/websocket-endpoint` is SecureString version 2 and contains the clean `wss:` endpoint without `apiKey`; `/bolt/polyresearch/api-key` remains the only PRR credential source.
- A no-send WebSocket frame probe proved no initial subscribe message is required: the stream emitted text JSON frames with key set `price,symbol,ts`, type map `price:number,symbol:string,ts:number`, symbols `BTC/USD`, `ETH/USD`, `SOL/USD`, `XRP/USD`, and zero parse errors.
- A 25-second no-send hold proved no application-level subscribe or heartbeat message was required in that window: connection opened, stayed open until timeout, emitted `92` schema-valid frames, and printed no raw values.
- Required cleanup before runtime provider code is complete: PRR runtime code must keep consuming `/bolt/polyresearch/api-key` as the only credential source and must not consume a credential-bearing endpoint URL as a second source.

## Slice Boundaries

- Slice A: finish #581 current branch only. Scope: eu-west-2 path parity, fail-loud reference parsing, and stale test/comment cleanup. No new provider architecture.
- Slice B: #579 gate deletion from fresh `main`. Scope: remove dead readiness/operator gate identity. No reference-price provider work.
- Slice C: proper reference-price architecture from fresh `main` after Slice A/B decisions. Scope: TOML-owned active reference source and comparator providers. No fastest-wins execution and no PRR `price_to_beat`.

---

### Task 1: Finish Eu-West-2 SSM Path Parity In Tests

**Files:**
- Modify: `tests/support/mod.rs`
- Modify: `src/bolt_v3_secrets.rs`
- Modify: `src/bolt_v3_providers/mod.rs`
- Modify: `tests/bolt_v3_client_registration.rs`
- Modify: `tests/bolt_v3_adapter_mapping.rs`
- Modify: `tests/bolt_v3_readiness.rs`
- Modify: `tests/bolt_v3_cli.rs`
- Modify: `tests/bolt_v3_operator_artifacts.rs`
- Modify: `tests/config_parsing.rs`

- [x] **Step 1: Apply the canonical test path mapping**

Use this exact mapping everywhere a fake resolver or test assertion expects current shipped Polymarket SSM names:

```rust
const POLY_PRIVATE_KEY_PATH: &str = "/bolt/polymarket/private-key";
const POLY_API_KEY_PATH: &str = "/bolt/polymarket/api-key";
const POLY_API_SECRET_PATH: &str = "/bolt/polymarket/api-secret";
const POLY_PASSPHRASE_PATH: &str = "/bolt/polymarket/api-passphrase";
```

- [x] **Step 2: Update shared fake resolver**

In `tests/support/mod.rs`, replace the old `fake_bolt_v3_resolver` Polymarket arms with:

```rust
POLY_PRIVATE_KEY_PATH => Ok(FAKE_BOLT_V3_POLYMARKET_PRIVATE_KEY.to_string()),
POLY_API_KEY_PATH => Ok("polymarket-api-key".to_string()),
POLY_API_SECRET_PATH => Ok("YWJj".to_string()),
POLY_PASSPHRASE_PATH => Ok("polymarket-passphrase".to_string()),
```

If the constants are not already in scope, keep them private in `tests/support/mod.rs` and use literal strings in other test files only where a local assertion must name the expected path.

- [x] **Step 3: Update module-test fake resolvers**

In `src/bolt_v3_secrets.rs` and `src/bolt_v3_providers/mod.rs`, update only `#[cfg(test)] mod tests` fake resolvers and expected path arrays from the legacy Polymarket namespace to:

```rust
"/bolt/polymarket/private-key"
"/bolt/polymarket/api-key"
"/bolt/polymarket/api-secret"
"/bolt/polymarket/api-passphrase"
```

Do not add production constants for these paths. Runtime values stay TOML-owned.

- [x] **Step 4: Update targeted test assertions**

Update these files so all fake SSM server maps, failing-path sentinels, redaction assertions, and path-received assertions use `/bolt/polymarket/*`:

```bash
rg -n "polymarket_main/(private_key|api_key|api_secret|passphrase)" tests src
```

Expected after edits: no matches in `tests` or `src`.

- [x] **Step 5: Verify registration**

Run:

```bash
cargo test --locked --test bolt_v3_client_registration -- --nocapture
```

Expected: all 6 tests pass.

- [x] **Step 6: Verify SSM/config surfaces**

Run:

```bash
cargo test --locked --test config_parsing shipped_polymarket_secrets_use_eu_west_2_registry_paths rejects_ssm_paths_missing_leading_slash rejects_ssm_paths_with_leading_or_trailing_whitespace -- --nocapture
cargo test --locked --test bolt_v3_cli bolt_v3_secrets_check_rejects_missing_provider_secret_field -- --nocapture
```

Expected: all named tests pass and no output exposes secret values.

---

### Task 2: Keep #581 Branch Narrow And Fail-Loud

**Files:**
- Modify: `tests/bolt_v3_client_registration.rs`
- Modify if needed: `src/strategies/binary_oracle_edge_taker/config.rs`
- Modify if needed: `tests/bolt_v3_strategy_registration.rs`

- [x] **Step 1: Correct stale comments**

In `tests/bolt_v3_client_registration.rs`, remove wording that says the fixture reference comes from `decision_reference`. Replace it with:

```rust
// The fixture has an empty [reference_data] block, so the scoped trade runner
// registers no live reference quote client. decision_reference is a logical
// gate identity and must not be treated as an NT data client.
```

- [x] **Step 2: Preserve malformed flat instrument rejection**

Keep the branch behavior that rejects malformed flat runtime IDs in `BinaryOracleEdgeTakerBuilder::parse_config`. The required regression assertion is:

```rust
assert!(
    rendered.contains("reference_instrument_id must be a valid NT instrument id"),
    "invalid flat reference instrument id must fail loud, got: {rendered}"
);
```

- [x] **Step 3: Preserve decision_reference separation**

Keep the regression that `raw_taker_config` omits `reference_venue` and `reference_instrument_id` when `[reference_data]` is empty and `decision_reference` exists. The expected table assertions are:

```rust
assert!(!table.contains_key("reference_venue"));
assert!(!table.contains_key("reference_instrument_id"));
```

- [x] **Step 4: Do not add an interim hardcoded reference**

Do not set `reference_data` from `underlying_asset` in Rust code. If an emergency config-only unblock is explicitly approved later, it must be TOML-only per strategy, for example:

```toml
[reference_data.primary]
data_client_id = "okx_data"
instrument_id = "BTC-USDT.OKX"
```

That emergency unblock must also remove `target.gate_subscriptions.decision_reference` from the same strategy file. It is not the proper forward architecture.

- [x] **Step 5: Verify #581 branch**

Run:

```bash
cargo test --locked --test bolt_v3_strategy_registration binary_oracle_runtime_mapping -- --nocapture
cargo test --locked --test bolt_v3_client_registration -- --nocapture
cargo test --locked --test config_parsing shipped_polymarket_secrets_use_eu_west_2_registry_paths -- --nocapture
cargo fmt --check
```

Expected: all commands exit 0.

---

### Task 3: Remove Dead Evidence Gate In A Separate #579 Branch

**Files:**
- Modify/delete: `src/bolt_v3_decision_evidence.rs`
- Modify/delete: `src/strategies/registry.rs`
- Modify/delete: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify/delete: `src/bolt_v3_operator_artifacts.rs`
- Modify/delete: `src/bolt_v3_live_node.rs`
- Modify/delete: `src/bolt_v3_config.rs`
- Modify/delete: `src/bolt_v3_canary_proof_executor.rs`
- Modify/delete: `src/bolt_v3_no_submit_readiness.rs`
- Modify/delete: `src/bolt_v3_tiny_canary_evidence.rs`
- Modify/delete: affected tests under `tests/`

- [ ] **Step 1: Start from fresh main**

Run:

```bash
git fetch origin
git worktree add .worktrees/579-remove-dead-gates origin/main
```

Expected: new clean worktree.

- [ ] **Step 2: Delete readiness evidence from strategy context**

Remove from `src/strategies/registry.rs`:

```rust
readiness_evidence: Option<BoltV3ReadinessGateEvidenceSnapshot>,
with_readiness_evidence(...)
readiness_evidence(...)
```

Replace tests that used `.with_readiness_evidence(...)` with direct post-decision evidence setup or remove them if they only prove gate identity.

- [ ] **Step 3: Remove gate identity from live strategy evidence**

In `src/strategies/binary_oracle_edge_taker/mod.rs`, remove the branch that records empty `gate_session_hash`, `selected_market_key`, and `gate_evidence` when readiness evidence is absent. Live strategy input evidence must not carry empty gate identity fields.

- [ ] **Step 4: Delete strict readiness gate validator**

Remove these from `src/bolt_v3_decision_evidence.rs`:

```rust
BoltV3ReadinessGateEvidenceSnapshot
validate_readiness_gate_evidence_snapshot
validate_strategy_input_readiness_evidence
from_entry_readiness_gate_session
```

Keep post-decision order intent/admission evidence that is not a pre-submit gate.

- [ ] **Step 5: Remove operator evidence config**

Delete `[live_canary.operator_evidence]` from `config/root.toml` and fixture TOMLs. Remove parser fields that exist only for this block. Keep live submit caps and kill-switch behavior owned by `BoltV3SubmitAdmissionState`.

- [ ] **Step 6: Confirm orphaned modes before deleting**

Run:

```bash
rg -n "bolt_v3_canary_proof_executor|bolt_v3_no_submit_readiness|bolt_v3_tiny_canary_evidence|operator_evidence|readiness_evidence|gate_session_hash|gate_evidence" src tests config
```

Delete modules and CLI subcommands only when the search proves they are used solely for the retired gate apparatus. Preserve market identity fields if they are still live-used outside gate identity.

- [ ] **Step 7: Verify #579 deletion**

Run:

```bash
cargo test --locked --test bolt_v3_submit_admission -- --nocapture
cargo test --locked --test bolt_v3_strategy_registration -- --nocapture
cargo test --locked --lib bolt_v3_decision_evidence -- --nocapture
cargo clippy --locked --all-targets -- -D warnings
just source-fence
```

Expected: all commands exit 0; non-test code has no `readiness_evidence`, `BoltV3ReadinessGateEvidenceSnapshot`, or `[live_canary.operator_evidence]` references.

---

### Task 4: Build Proper Reference-Price Provider Architecture

**Files:**
- Create: `src/bolt_v3_reference_price.rs`
- Modify: `src/bolt_v3_config.rs`
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Create: `src/bolt_v3_providers/chainlink_reference.rs`
- Create: `src/bolt_v3_providers/polyresearch.rs`
- Modify: `src/bolt_v3_providers/mod.rs`
- Modify: `src/lib.rs`
- Create: `tests/bolt_v3_reference_price.rs`

- [x] **Step 0: Close PRR handshake/auth risk before runtime provider code**

Before writing `src/bolt_v3_providers/polyresearch.rs`, obtain non-secret evidence for the PRR WebSocket handshake:

```text
websocket URL source: /bolt/polyresearch/websocket-endpoint in eu-west-2 SSM
credential source: /bolt/polyresearch/api-key in eu-west-2 SSM
credential placement: pre-cleanup probe found apiKey in the endpoint query; cleaned runtime contract stores endpoint version 2 without apiKey and attaches the separate api-key value exactly once
subscription shape: no initial subscribe message required; stream emits all supported symbols after connect
heartbeat/reconnect: no application heartbeat required over a 25-second no-send hold; provider should still use the NT websocket client's normal reconnect/transport heartbeat support
```

Evidence source: live schema/handshake probes that printed only endpoint shape, status codes, header names, equality booleans, field names, and frame key sets. They did not print credential values, raw endpoint values, or raw frames containing prices.

Stop this task if the handshake requires an environment variable, local file, 1Password runtime lookup, AWS CLI subprocess, Python bridge, or any secret source outside the existing Rust SSM resolver path.

- [ ] **Step 1: Add TOML-owned root reference_price config**

Add:

```toml
[reference_price]
active_source = "chainlink_ws"
comparators = ["prr_ws"]
max_active_age_ms = 1500
max_comparator_drift_bps = 10
drift_policy = "block"
comparator_health_policy = "observe"
```

Validation rules:

```text
active_source in {"chainlink_ws", "prr_ws"}
comparators contains known providers only
active_source not listed in comparators
no duplicate comparators
max_active_age_ms > 0
max_comparator_drift_bps > 0 and finite
drift_policy in {"observe", "block"}
comparator_health_policy in {"observe", "require"}
```

`[reference_price]` is root-owned and copied into each strategy runtime config as a complete object. Do not add flat per-strategy `reference_price_*` fields. Strategy validation combines this root policy with each strategy's `target.underlying_asset`; PRR active or PRR comparator is rejected for BNB/DOGE.

- [ ] **Step 2: Add normalized reference quote model**

Create `ReferenceQuote` with provider-specific provenance and implement NT custom data traits so providers publish it as `Data::Custom(CustomData)`:

```rust
pub const REFERENCE_QUOTE_TYPE_NAME: &str = "BoltV3ReferenceQuote";

pub struct ReferenceQuote {
    pub provider_id: String,
    pub symbol: String,
    pub price: f64,
    pub source_timestamp_ms: u64,
    pub local_receive_timestamp_ms: u64,
    pub provenance: ReferenceQuoteProvenance,
}

pub enum ReferenceQuoteProvenance {
    Chainlink {
        feed_id: String,
        valid_from_timestamp_ms: u64,
        observations_timestamp_ms: u64,
        full_report_sha256: String,
    },
    Prr {
        symbol: String,
        ts_ms: u64,
    },
}
```

Implement `CustomDataTrait` for `ReferenceQuote`, register JSON deserialization if catalog/replay coverage is required, and construct `DataType::new(REFERENCE_QUOTE_TYPE_NAME, metadata, Some(symbol))` for subscription. Reject non-positive or non-finite prices before state update.

- [ ] **Step 3: Keep PRR asset selection config-owned**

Do not ship a compiled PRR supported-asset allowlist. The operator-owned
`[reference_current_price.source.<id>].symbol` value defines the subscribed
asset, and validation must fail only when that configured symbol does not map to
`reference_current_price.asset`.

- [ ] **Step 4: Keep price_to_beat separate**

Do not modify the Chainlink strike source that emits `IndexPriceUpdate` for `price_to_beat`. Add a regression test proving PRR reference quotes cannot set `price_to_beat`:

```rust
#[test]
fn prr_reference_quote_never_sets_price_to_beat() {
    let mut strategy = binary_oracle_edge_taker_for_test();
    strategy.observe_reference_quote(&fast_spot("prr_ws", 100.0, 1_000));
    assert!(strategy.active.price_to_beat.is_none());
}
```

- [ ] **Step 5: Wire active source and comparators through NT custom data**

Provider bindings:

```text
src/bolt_v3_providers/chainlink_reference.rs
  KEY = "CHAINLINK_DATA_STREAMS_REFERENCE"
  data-only provider
  secrets: api_key_ssm_parameter, api_secret_ssm_parameter
  transport: nautilus_network::websocket::WebSocketClient
  emits: Data::Custom(CustomData::new(ReferenceQuote, DataType))

src/bolt_v3_providers/polyresearch.rs
  KEY = "POLYRESEARCH_REFERENCE"
  data-only provider
  secrets: api_key_ssm_parameter, websocket_endpoint_ssm_parameter
  transport: nautilus_network::websocket::WebSocketClient
  emits: Data::Custom(CustomData::new(ReferenceQuote, DataType))
```

Register both bindings in `PROVIDER_BINDINGS`. Each provider's `map_adapters` returns a `BoltV3DataClientAdapterConfig` with a provider-owned `DataClientFactory` and config.

Strategy wiring:

```text
on_start:
  subscribe_data(reference_quote_data_type(asset, active_source), Some(active_client_id), None)
  subscribe_data(reference_quote_data_type(asset, comparator_source), Some(comparator_client_id), None)

on_data:
  downcast CustomData to ReferenceQuote
  reject wrong asset symbol
  update ReferencePriceState
  adapt only the configured active source to FastSpotObservation
  call observe_reference_quote only for active-source quote
```

Comparator quotes update drift/cadence/staleness health only. Arrival order is telemetry and must never select the trading source. Do not route PRR or Chainlink reference WebSocket data through `on_quote`.

- [x] **Step 6: Resolve PRR credentials only from SSM and remove the current duplicate key representation**

PRR config must reference eu-west-2 SSM names through the Rust secret resolver path. The approved names verified by name-only inventory are:

```text
/bolt/polyresearch/api-key
/bolt/polyresearch/websocket-endpoint
```

Completed SSM repair: `/bolt/polyresearch/websocket-endpoint` is SecureString version 2 and stores the clean `wss:` endpoint without `apiKey`; `/bolt/polyresearch/api-key` remains the only PRR credential source. Runtime code must attach `apiKey` exactly once when constructing the WebSocket URL and must reject or avoid credential-bearing endpoint URLs so the key is not consumed twice.

No 1Password, environment variable, local file, AWS CLI subprocess, or Python fallback is allowed in runtime code.

The PRR endpoint path is treated as sensitive config and resolved through the same provider secret-resolution layer as the API key. The provider config/debug output must redact both values.

- [ ] **Step 7: Verify reference-price architecture**

Run:

```bash
cargo test --locked --test bolt_v3_reference_price -- --nocapture
cargo test --locked --test bolt_v3_strategy_registration binary_oracle_runtime_mapping -- --nocapture
cargo clippy --locked --all-targets -- -D warnings
just source-fence
```

Expected:

- Active-source quote updates trading reference state.
- Comparator quote does not replace active source.
- PRR cannot bind `price_to_beat`.
- PRR is rejected for BNB/DOGE.
- No non-SSM secret source exists.
- No fastest-wins behavior exists.

---

### Task 5: Live Verification Only After Code Is Green

**Files:**
- No code files.

- [ ] **Step 1: Confirm local and CI gates**

Do not run a live canary until exact PR head has:

```bash
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
just source-fence
```

Expected: all commands exit 0.

- [ ] **Step 2: Confirm eu-west-2 SSM names without values**

Run name-only checks:

```bash
aws ssm describe-parameters --region eu-west-2 --parameter-filters Key=Name,Option=BeginsWith,Values=/bolt/polymarket/ --query 'Parameters[].Name' --output text
aws ssm describe-parameters --region eu-west-2 --parameter-filters Key=Name,Option=BeginsWith,Values=/bolt/polyresearch/ --query 'Parameters[].Name' --output text
aws ssm describe-parameters --region eu-west-2 --parameter-filters Key=Name,Option=BeginsWith,Values=/bolt/testnet/chainlink/ --query 'Parameters[].Name' --output text
```

Expected: required names present; no secret values printed.

- [ ] **Step 3: Run live debug only with explicit approval**

Only after user approval, run a short SSM debug start/stop on the target instance and restore service state. Required postcondition:

```text
bolt-v2.service inactive/disabled
RUST_LOG unset
no persistent debug override left behind
```

Expected live evidence:

- reference provider connects and subscribes,
- active reference quote updates `spot_price` / `reference_fair_value`,
- interval open warms only after source-bound `price_to_beat`,
- entry blocks clear or report a new concrete blocker.
