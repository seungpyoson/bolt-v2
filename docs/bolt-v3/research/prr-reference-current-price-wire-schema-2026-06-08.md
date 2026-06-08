# PRR Reference Current Price Wire Schema - 2026-06-08

## Scope

This note records non-secret evidence for implementing PRR as a Bolt v3
reference-current-price provider. It covers connection/auth shape, subscribe
messages, price-feed event fields, timestamp units, heartbeat/keepalive, and
unsubscribe behavior.

This note does not contain API keys, raw credentialed URLs, or live captured
payloads.

## Secret Safety

- Credential values remain in AWS SSM and must be resolved through the existing
  Rust SSM path.
- Runtime code must not read PRR credentials from environment variables,
  1Password, local files, AWS CLI subprocesses, or Python helpers.
- Logs and evidence may name config fields and SSM parameter names, but must not
  print credential values or credentialed URLs.

## Evidence

Authoritative PRR documentation was reviewed on 2026-06-08:

- PolyNode WebSocket overview:
  https://docs.polynode.dev/websocket/overview
- PolyNode WebSocket subscriptions and filters:
  https://docs.polynode.dev/websocket/subscribing
- PolyNode price feed event reference:
  https://docs.polynode.dev/websocket/events/price-feed
- PolyNode crypto price feed event format:
  https://docs.polynode.dev/crypto/event-format
- PolyNode available crypto feeds:
  https://docs.polynode.dev/crypto/feeds

The docs prove the following runtime contract:

| Area | Source-proven shape |
| --- | --- |
| Connection URL | `wss://ws.polynode.dev/ws` with API key supplied as query parameter `key`. |
| Accepted key prefixes | API keys starting with `pn_live_` or `qm_live_` are accepted. |
| Subscribe message | JSON text frame with `action = "subscribe"` and `type = "chainlink"`. |
| Feed filter | Optional `filters.feeds` array such as `["BTC/USD", "ETH/USD"]`; omitted feeds subscribe all price feeds. |
| Subscribe response | `type = "subscribed"`, with subscription metadata. Price feeds do not send an initial snapshot. |
| Price event | `type = "price_feed"`, top-level `feed`, top-level `timestamp`, and `data` object. |
| Data fields | `data.feed`, `data.price`, `data.bid`, `data.ask`, `data.timestamp`. |
| Timestamp unit | Unix seconds for price-feed observations. |
| Heartbeat | WebSocket ping plus text heartbeat frames. Application `{"action":"ping"}` receives `{"type":"pong", "ts": ...}`. |
| Unsubscribe | `{"action":"unsubscribe", "subscription_id":"..."}` or `{"action":"unsubscribe"}` for all subscriptions. |
| Available feeds | `BTC/USD`, `ETH/USD`, `SOL/USD`, `BNB/USD`, `XRP/USD`, `DOGE/USD`, and `HYPE/USD`. |

## Runtime Contract

Bolt must implement PRR reference-current-price ingestion as a normal NT data
client:

1. Build the credentialed WebSocket URL at the PRR provider edge by appending the
   SSM-resolved API key as query parameter `key`.
2. Send a provider subscribe frame for the configured source:

   ```json
   {"action":"subscribe","type":"chainlink","filters":{"feeds":["BTC/USD"]}}
   ```

3. Parse only source-proven price-feed events.
4. Convert Unix-second timestamps to millisecond timestamps before constructing
   `ReferencePriceUpdate`.
5. Map the PRR feed name back to the configured provider symbol and normalized
   `reference_current_price.asset`.
6. Emit `Data::Custom(ReferencePriceUpdate)` through the same
   `SubscribeCustomData` path as all reference-current-price providers.
7. Ignore heartbeat, pong, subscribed, unsubscribed, and non-price event frames
   without mutating trading state.
8. Treat malformed price events, non-finite prices, non-positive prices, and
   wrong-feed events as non-updates.

## Implementation Decision

PRR parsing is source-proven and approved for implementation.

The local helper and tests now use the source-proven query parameter name
`key`. Runtime URL construction must append `key` exactly once, reject endpoints
already containing `key`, and reject the legacy credential query `apiKey`.

## Stop Conditions

- Stop PRR runtime implementation if live PRR rejects `key` and source-proven
  updated docs or provider confirmation cannot be obtained.
- Stop if PRR requires a credential source other than AWS SSM through the Rust
  resolver.
- Stop if the provider frame format changes away from the documented
  `price_feed` contract before implementation lands.
