# Chainlink Data Streams — Testnet Price Feed Guide

> Verified 2026-02-28 against Chainlink testnet with live WebSocket and REST tests.

> **TESTNET IS PRODUCTION.** For bolt-v2, mainnet/production Chainlink Data Streams
> credentials cannot be obtained — this testnet feed is the only and final
> resolution anchor for `price_to_beat`. Treat it as production. Do not gate,
> hesitate, or argue testnet-vs-mainnet when deploying/running the live strike.
> The deployed runtime config may read `environment = "mainnet"`, but the strike
> resolves off testnet because that is all that exists.

## Architecture Overview

Chainlink Data Streams is a **pull-based, off-chain** price oracle. Unlike on-chain Chainlink Price Feeds (which read from aggregator contracts on Ethereum/Arbitrum), Data Streams delivers signed price reports directly via **WebSocket** or **REST API** — no blockchain interaction needed for reading prices.

```
Chainlink DON (16 oracle operators)
        │
        ▼
Data Engine (Chainlink-hosted)
        │
    ┌───┴───┐
    WS API  REST API
    │       │
    ▼       ▼
  Bolt    ad-hoc queries
```

Each price update is a **V3 report** containing: benchmark price (mid), bid, ask, timestamps, and fees — signed by the oracle network. Reports can optionally be verified on-chain.

## Testnet Endpoints

| Protocol | URL |
|----------|-----|
| **WebSocket** | `wss://ws.testnet-dataengine.chain.link` |
| **REST API** | `https://api.testnet-dataengine.chain.link` |

**Critical**: Testnet feed IDs are completely different from mainnet feed IDs. The root cause of Bolt's dead Chainlink feed was using mainnet BTC feed ID `0x00039d9e...b8` against the testnet endpoint — it silently returned zero data (WS connected fine but sent no messages; REST returned `404 report not found`).

## Feed IDs

### Testnet feed-id bindings (single source of truth)

The authoritative per-asset testnet `feed_id → resolution-instrument` bindings for the **live
strike path** live in `config/root.toml` under the live strike client's
`[clients.chainlink_strike.data].feed_bindings` — that is the only source the running strategy
loads. The `chainlink_data_streams` gate provider's `feed_bindings` is a separate **offline**
proof stub (a single placeholder, retired in #551); it is **not** the live strike source, so
editing it does not change live behavior. Bindings are intentionally **not duplicated here**:
`config/root.toml` is the single source of truth, so this guide cannot drift from what the
runtime actually loads, and the live per-asset bindings are pinned by a drift-guard test
(`shipped_chainlink_strike_client_pins_each_asset_feed_binding` in `tests/config_parsing.rs`)
so a cross-asset feed swap fails CI. Each `feed_id` was discovered empirically and verified
against the testnet endpoint using the procedure below (crypto feeds update at 1 Hz). Mainnet
feed IDs differ and are a config swap, not a code change.

### How to Discover Testnet Feed IDs for New Assets

There is **no metadata API** that maps feed IDs to human-readable names. Chainlink returns only raw hex IDs. Testnet feed IDs must be discovered empirically:

1. Call `GET /api/v1/feeds` — returns all available feed IDs (510 as of Feb 2026, 211 with `0x0003` crypto prefix)
2. For each candidate, call `GET /api/v1/reports/latest?feedID={id}` to get its current price
3. Match prices against known market rates (e.g., BTC ~$66K, ETH ~$1.9K, SOL ~$85, XRP ~$1.40)
4. Verify via WebSocket stream to confirm 1 Hz updates

Feed ID prefix convention:
- `0x0003` — Crypto price feeds (BTC, ETH, SOL, XRP, etc.)
- `0x000b` — Other feeds (forex, commodities, indices)
- `0x0007` — Additional feed types

## Authentication

All API access requires HMAC-SHA256 authentication with an API key + API secret pair.

### Credential Storage (Bolt)

SSM Parameter Store path: `/bolt/testnet/chainlink/`
- `api-key` (36 chars, UUID format)
- `api-secret` (128 chars, hex)
- Region: `eu-west-2` (creds replicated to eu-west-2 on 2026-06-20; the eu-west-1 copy was deleted in the region decommission)

### HMAC Signing Process

Three custom headers are required on every request (WebSocket upgrade or REST call):

```
Authorization: {api_key}
X-Authorization-Timestamp: {unix_timestamp_ms}
X-Authorization-Signature-SHA256: {hmac_hex}
```

The signature is computed as:

```
signing_string = "{METHOD} {path_with_query} {sha256_of_body} {api_key} {timestamp_ms}"
signature = HMAC-SHA256(api_secret, signing_string)
```

For GET requests and WebSocket upgrades, the body hash is the SHA-256 of empty string:
```
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
```

A `User-Agent` header is also required — Cloudflare returns error 1010 without one.

### Signing Pitfall

The `ws_url` in config must NOT have a trailing slash. A trailing slash causes the HMAC to sign `/api/v1/ws/?feedIDs=...` (double slash) while the server expects `/api/v1/ws?feedIDs=...` — producing a silent auth failure.

## WebSocket API

### Connection

```
wss://ws.testnet-dataengine.chain.link/api/v1/ws?feedIDs={id1},{id2},{id3}
```

Multiple feed IDs can be comma-separated in a single connection. Auth headers are sent during the HTTP upgrade handshake.

### Message Format

Each message is JSON with a `report` field:

```json
{
  "report": {
    "feedID": "0x00037da0...",
    "validFromTimestamp": 1772203793,
    "observationsTimestamp": 1772203793,
    "fullReport": "0x00090d9e..."
  }
}
```

Messages arrive at **1 Hz per feed** (1 update per second). A connection subscribing to 4 feeds receives ~4 messages/second.

### Reconnection

On disconnect, reconnect with exponential backoff. Auth headers must be regenerated (new timestamp + signature) on each reconnect.

## REST API

### List Available Feeds

```
GET /api/v1/feeds
-> {"feeds": [{"feedID": "0x..."}, ...]}
```

Returns all feed IDs available on testnet. No metadata (names, descriptions) included.

### Get Latest Report

```
GET /api/v1/reports/latest?feedID={feed_id}
-> {"report": {"feedID": "...", "fullReport": "0x...", ...}}
```

Returns 404 `{"message": "report not found"}` if the feed ID doesn't exist on testnet.

## Decoding V3 Reports

The `fullReport` hex string is an ABI-encoded structure containing signatures and the price report blob.

### Outer Structure (ABI-encoded)

```
Offset    Field
------    -----
0-96      reportContext (bytes32[3]) — config digest, epoch+round, etc.
96-128    offset pointer to reportBlob -> typically 224
128-160   offset pointer to rawRs
160-192   offset pointer to rawSs
192-224   rawVs (bytes32)
224-256   reportBlob length (e.g., 288)
256+      reportBlob data
```

### Report Blob (V3 Schema, 288 bytes)

```
Offset    Type      Field
------    ----      -----
0-32      bytes32   feedId
32-64     uint32    validFromTimestamp (right-padded)
64-96     uint32    observationsTimestamp
96-128    uint192   nativeFee
128-160   uint192   linkFee
160-192   uint32    expiresAt (~30 days from now)
192-224   int192    benchmarkPrice (mid) — 18 decimal places
224-256   int192    bid — 18 decimal places
256-288   int192    ask — 18 decimal places
```

### Price Extraction

```
benchmarkPrice (raw int) / 10^18 = USD price
```

Example: raw `65785240335769465000000` -> `$65,785.24`

### Decode Steps (pseudocode)

```
raw = hex_decode(fullReport[2:])        # strip "0x" prefix
blob_offset = uint256(raw[96:128])      # read offset pointer
blob_length = uint256(raw[blob_offset:blob_offset+32])
blob = raw[blob_offset+32 : blob_offset+32+blob_length]
price = int192(blob[192:224]) / 1e18    # benchmark (mid)
bid   = int192(blob[224:256]) / 1e18
ask   = int192(blob[256:288]) / 1e18
```

**Pitfall**: Do NOT use `lstrip("0x")` to strip the hex prefix — Python's `lstrip` strips all matching characters, not just the prefix. Use `[2:]` slicing instead. `"0x0003...".lstrip("0x")` strips leading `0`, `x`, `0`, `0` and corrupts the data.

## Common Failure Modes

| Symptom | Cause | Fix |
|---------|-------|-----|
| WS connects, zero messages | Wrong feed ID (e.g., mainnet ID on testnet) | Use the testnet feed IDs listed above |
| REST returns 404 `report not found` | Feed ID doesn't exist on testnet | Same as above |
| WS connection rejected (401) | Bad API key/secret or wrong HMAC signature | Check credentials; verify signing string format |
| WS connection rejected (403) | Key not authorized for this feed | Contact Chainlink for feed access |
| Cloudflare error 1010 | Missing User-Agent header | Add `User-Agent: bolt-data-collector/1.0` or similar |
| Price decodes to $0.00 | Wrong ABI offset in decode logic | Use `blob_offset = uint256(raw[96:128])` without adding 96 |
| Hex decode error at position ~1469 | Used `lstrip("0x")` instead of `[2:]` | Use slice `fullReport[2:]` to strip `0x` prefix |
| HMAC signature mismatch | Trailing slash in ws_url | Normalize: `ws_url.trim_end_matches('/')` |

## Bolt Integration (bolt-v2 / bolt-v3)

> The original guide described the bolt-v1 layout (`[feeds.chainlink]` TOML,
> `src/feeds/chainlink.rs`). bolt-v2 does **not** use that shape. In bolt-v2 the
> strike feeds the binary-oracle `price_to_beat` through a NautilusTrader
> provider binding, and the strike source is a **point-in-time REST
> boundary-fetch** `DataClient` (it fetches the report AT the market's
> interval-open second and emits one NT `IndexPriceUpdate`) — not a continuous
> WebSocket stream. The WebSocket section above is protocol reference; the live
> strike path uses the REST `reports` endpoint.

### TOML Config (bolt-v2 shape)

The Chainlink strike client is a normal NT data client under `[clients.<id>]`,
with one `feed_bindings` entry per supported asset mapping `feed_id` to the NT
resolution instrument. Secrets are SSM parameter names only (resolved at
runtime, never stored in TOML). The authoritative schema is owned by the code
(`parse_feed_binding` / `ChainlinkDataConfig` in
`src/bolt_v3_providers/chainlink.rs` +
`src/bolt_v3_providers/chainlink/strike_source.rs`)
and the shipped values live in `config/root.toml` — this guide does not restate
them so they cannot drift. Shape:

```toml
[clients.<id>.data]
rest_base_url = "https://api.testnet-dataengine.chain.link"
report_endpoint_path = "/api/v1/reports"
http_timeout_secs = <n>

[[clients.<id>.data.feed_bindings]]
feed_id = "<0x + 64 lowercase hex testnet feed id from the table above>"
instrument_id = "<NT resolution instrument>"
report_schema_version = 3
report_decimal_scale = 18
price_precision = <u8>

[clients.<id>.secrets]
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"
```

The strategy selects its feed per its own `target.underlying_asset` (one
strategy instance = one asset), so the binding follows whichever market that
instance trades.

### Strategy Subscription Contract (`price_to_beat`)

For the current binary-oracle strategy, `price_to_beat` is selected by the
strategy's `[resolution_data]` block:

```toml
[resolution_data]
data_client_id = "chainlink_strike"
instrument_id = "BTC-USD.CHAINLINK"
```

That block does not contain a Chainlink `feed_id`. `data_client_id` selects the
root `[clients.chainlink_strike]` data client, and `instrument_id` selects the
matching `[[clients.chainlink_strike.data.feed_bindings]]` row in
`config/root.toml`.

At load time, the binary-oracle archetype bridge validates the binding and
copies it into the raw strategy config as `resolution_client_id` and
`resolution_instrument_id`. The current validation requires this client to be
the Chainlink Data Streams strike client and requires the instrument to have a
matching `feed_bindings` entry. This means the feed IDs are config-driven, but
the current `price_to_beat` subscription path is Chainlink strike-source
specific.

At runtime, the strategy subscribes to the strike like this:

1. When an interval is selected and `price_to_beat` is still unset, the strategy
   calls `subscribe_resolution_strike`.
2. The strategy computes `window_open_unix_seconds` from the market interval
   open timestamp.
3. The strategy calls NT `subscribe_index_prices` with the configured
   `resolution_instrument_id`, the configured `resolution_client_id`, and a
   params map containing `window_open_unix_seconds`.
4. The Chainlink strike source receives that subscribe command, maps the
   resolution instrument to its configured `feed_id`, fetches one REST report for
   that window-open timestamp, and emits one NT `IndexPriceUpdate`.
5. The strategy accepts only an `IndexPriceUpdate` for its configured
   `resolution_instrument_id`; if the update timestamp matches the selected
   interval open and the value is positive finite, it binds that value as
   `price_to_beat`.

This is a point-in-time resolution-strike path, not a continuous reference-price
feed. Do not route `price_to_beat` through `[reference_data]`,
`decision_reference`, PRR WebSocket quotes, or future normalized reference-quote
providers. Those belong to live fair-value/reference pricing and must remain
separate from the market boundary strike.

Code landmarks for future sessions:

- Loader injection and validation:
  `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`
- Strategy subscribe and receive path:
  `src/strategies/binary_oracle_edge_taker/mod.rs`
  (`subscribe_resolution_strike`, `on_index_price`)
- Chainlink subscribe handler:
  `src/bolt_v3_providers/chainlink/strike_source.rs`
  (`ChainlinkStrikeSourceClient::subscribe_index_prices`)

### Adding a New Asset

1. Probe all `0x0003`-prefix feeds via REST: list `GET /api/v1/feeds`, then `GET /api/v1/reports/latest?feedID={id}` for each and decode the mid price
2. Match the returned price against the asset's known market price
3. Verify 1 Hz WebSocket stream before deploying
4. Add a `[[clients.<id>.data.feed_bindings]]` entry (feed_id + resolution instrument) in `config/root.toml`, and add the feed id to the verified-testnet table above

### Source Code (bolt-v2)

- Strike source (REST boundary fetch + emit `IndexPriceUpdate`): `src/bolt_v3_providers/chainlink/strike_source.rs`
- HMAC auth (signed request URL + headers): `src/bolt_v3_providers/chainlink/auth.rs`
- V3 `fullReport` decode: `src/bolt_v3_providers/chainlink/report.rs`
- Provider binding + config schema (`ChainlinkDataConfig`, `parse_feed_binding`): `src/bolt_v3_providers/chainlink.rs`
- SSM secret loading: `src/bolt_v3_providers/chainlink.rs` via `src/bolt_v3_secrets.rs`

---

## Future: Migration to Production (NOT ACTIVE — we only have testnet credentials)

When production credentials are obtained, three things change simultaneously — all must be updated together:

1. **Endpoints** change:
   - WS: `wss://ws.testnet-dataengine.chain.link` -> `wss://ws.dataengine.chain.link`
   - REST: `https://api.testnet-dataengine.chain.link` -> `https://api.dataengine.chain.link`

2. **Credentials** change:
   - New SSM path: `/bolt/prod/chainlink` (with production API key/secret)
   - TOML: `ssm_prefix = "/bolt/prod/chainlink"`

3. **Feed IDs** change — production uses completely different IDs:

   | Asset | Production Feed ID |
   |-------|--------------------|
   | BTC/USD | `0x00039d9e45394f473ab1f050a1b963e6b05351e52d71e507509ada0c95ed75b8` |
   | ETH/USD | `0x000362205e10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3ae9` |
   | SOL/USD | `0x0003b778d3f6b2ac4991302b89cb313f99a42467d6c9c5f96f57c29c0d2bc24f` |
   | XRP/USD | `0x0003c16c6aed42294f5cb4741f6e59ba2d728f0eae2eb9e6d3f555808c59fc45` |

   Production feed IDs can be looked up at: `https://data.chain.link/streams/{asset}-usd-cexprice-streams`

Mixing any of these (e.g., testnet feed IDs on production endpoint) will silently fail — the same way our testnet setup was broken when it had mainnet feed IDs.

---

## References

- Chainlink Data Streams overview: https://docs.chain.link/data-streams
- WebSocket API reference: https://docs.chain.link/data-streams/reference/data-streams-api/interface-ws
- REST API reference: https://docs.chain.link/data-streams/reference/data-streams-api/interface-api
- Authentication: https://docs.chain.link/data-streams/reference/data-streams-api/authentication
- Rust SDK tutorial: https://docs.chain.link/data-streams/tutorials/rust-sdk-stream
- Bolt-v2 strike source: `src/bolt_v3_providers/chainlink/` (auth.rs, report.rs, strike_source.rs)
- Bolt-v2 provider binding + config schema: `src/bolt_v3_providers/chainlink.rs`
