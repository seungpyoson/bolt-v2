# PRR And Chainlink Source Semantics - 2026-06-05

## Question

Can bolt-v2 listen to both Chainlink Data Streams WebSocket and PRR WebSocket, use whichever arrives first, and treat the value as equivalent trading input?

## Finding

Listen to both, but do not let "fastest wins" drive execution. The two streams do not have equivalent semantics for the values bolt-v2 cares about.

- PRR currently proves: live reference-price tick for a symbol, provider timestamp, local receive time.
- Chainlink Data Streams proves: Data Streams feed identity, signed report payload, report timestamps, and decoded schema fields.
- `price_to_beat` needs boundary provenance, not just a numeric price.
- PRR supports BTC/USD, ETH/USD, SOL/USD, and XRP/USD in the current feed. It does not cover BNB or DOGE.

## Decision

Role-specific decision after the 2026-06-05 live probe:

- PRR as `price_to_beat` source: **No**. Current PRR frames do not include Chainlink report identity, signed report payload, or boundary timestamps.
- PRR as automatic "fastest wins" execution input: **No**. The streams are price-aligned but semantically different.
- PRR as explicit continuous reference-price source for BTC/ETH/SOL/XRP: **Yes**, if configured as the active reference source and monitored against Chainlink. It should not be an implicit fallback.
- PRR for BNB/DOGE: **No**. PRR does not currently provide those symbols.

## Evidence

### PRR Setup Document

The PRR setup guide describes the feed as live reference prices for BTC/USD, ETH/USD, SOL/USD, and XRP/USD. It documents each message as one JSON object with exactly:

```json
{ "symbol": "BTC/USD", "price": 80814.27, "ts": 1778407880.123 }
```

The documented fields are:

- `symbol`
- `price`
- `ts`

The setup document does not document Chainlink feed id, `validFromTimestamp`, `observationsTimestamp`, `fullReport`, report hash, report schema, or signature material.

### Live PRR Schema Probe

A short live schema-only probe observed 12 PRR frames without printing credentials or raw price values:

```text
frames_observed=12
transport=text
key_sets=price,symbol,ts
type_maps=price:number,symbol:string,ts:number
symbols=BTC/USD,ETH/USD,SOL/USD,XRP/USD
extra_keys=[]
ts_fractional=True
raw_values_printed=false
```

This transient schema check was not archived as a separate raw log. The archived overlap log below provides the durable schema evidence and confirms the live PRR frames match the setup document: text WebSocket frames with only `symbol`, `price`, and `ts`.

### Archived Side-By-Side Probe

The archived 300-second side-by-side probe is saved at:

- `docs/bolt-v3/research/chainlink-prr-latency-2026-06-05.md`
- `docs/bolt-v3/research/chainlink-prr-latency-2026-06-05.raw.log`

Relevant evidence:

- PRR WebSocket delivered text messages: `text_messages=1180`, `binary_messages=0`.
- Chainlink WebSocket delivered binary report messages: `text_messages=0`, `binary_messages=1196`.
- PRR arrived first slightly more often in matched source-second buckets.
- Chainlink WebSocket had fewer long local receive gaps in that local run.

That probe supports using both streams for race/cadence evidence. It does not prove PRR is semantically equivalent to a Chainlink boundary report. The archived run and the current run disagree on which provider arrived first more often; that is why arrival order stays telemetry instead of execution policy.

### Current PR #569 ID Review

PR #569 has two Chainlink ID sets:

- `config/root.toml` contains the active testnet live-strike feed IDs loaded by the runtime.
- The guide's BTC/ETH/SOL/XRP table is in the "Future: Migration to Production" section and is not valid against testnet.

Live validation against Chainlink testnet:

- The six PR config IDs for BTC, ETH, SOL, BNB, XRP, and DOGE returned REST `200`, carried `fullReport`, and decoded back to the same feed ID.
- The same six config IDs delivered WebSocket reports for 15 seconds: `BTC:15, ETH:15, SOL:15, BNB:15, XRP:15, DOGE:15`.
- The four production-section IDs returned REST `404` against the testnet endpoint, as expected.

### Current Overlap Probe

The current 300-second overlap probe used the PR #569 testnet config IDs and only the four symbols PRR supports: BTC, ETH, SOL, XRP. Raw values, full reports, credentials, and endpoints were not printed. The raw sanitized log is saved at:

- `docs/bolt-v3/research/chainlink-prr-ws-overlap-2026-06-05.raw.log`

Summary:

- Chainlink WS connected and emitted `1196` binary messages; `1196` reports decoded successfully; parse errors `0`.
- PRR WS connected and emitted `1172` text messages; parse errors `0`.
- Chainlink schema: `feedID,fullReport,observationsTimestamp,validFromTimestamp`.
- PRR schema: `price,symbol,ts`.
- Chainlink cadence was steadier: each symbol had one `>1500ms` gap and zero `>2000ms` gaps.
- PRR had more long gaps: `19-22` `>1500ms` gaps per symbol and `2-4` `>2000ms` gaps per symbol.
- PRR source lag was lower by provider timestamp: median `86-87ms` vs Chainlink median `382-397ms`.
- In matched source-second buckets, Chainlink arrived first more often: `62.7-64.4%`; PRR arrived first `35.6-37.3%`.
- Absolute price drift was small for reference-price purposes in this run: median `0.2-0.6 bps`, p95 `1.4-3.2 bps`, max `2.8-6.0 bps`.

### Chainlink Data Streams Documentation

Chainlink's Data Streams docs describe WebSocket reports as report objects containing `feedID` and `fullReport`. The Crypto Advanced v3 report schema includes:

- `feedId`
- `validFromTimestamp`
- `observationsTimestamp`
- `price`
- `bid`
- `ask`
- fees and expiration fields

The docs also describe Data Streams as reports that can be fetched or streamed and verified.

## Interpretation

### Reference Price

Reference price is continuous live market state used for pricing, edge calculation, drift checks, and freshness.

PRR is usable as an explicit continuous reference-price source for BTC/ETH/SOL/XRP when the configured consumer only needs `symbol`, `price`, and `ts`. It should still be monitored against Chainlink because the current probe showed weaker cadence and no report provenance.

### Price To Beat

`price_to_beat` is the interval-open boundary value. A numeric price alone is insufficient because we need to prove the selected value is the report that opened the market interval.

For `price_to_beat`, the required evidence is:

- configured source identity
- feed id
- report schema
- report timestamp semantics
- exact boundary match: `valid_from == window_open`
- stale-data rejection
- source identity in operator artifacts

Chainlink Data Streams reports provide the necessary fields. Current PRR frames do not.

## How To Use Both

### Safe Now

Run both streams in one process and record:

- provider
- symbol
- provider timestamp
- local receive timestamp
- inter-arrival intervals
- stale duration
- gaps
- reconnects
- price drift between providers
- which provider arrived first for matched source-time buckets

This answers latency and reliability questions without changing execution semantics.

### Safe After Soak

Add explicit TOML source modes:

```toml
reference_price_source = "chainlink_ws"
reference_price_comparators = ["prr_ws"]
```

or, after evidence supports it:

```toml
reference_price_source = "prr_ws"
reference_price_comparators = ["chainlink_ws"]
```

The strategy should consume only the configured active reference source, while recording comparator drift and freshness.

### Not Safe Yet

Do not implement:

```text
if Chainlink is late, use PRR for price_to_beat
```

Current PRR frames do not prove boundary equivalence. If Chainlink cannot provide a valid boundary report, entry should stay blocked.

## What To Ask PRR

Before PRR can be considered for `price_to_beat`, ask whether they can expose any of:

- Chainlink feed id
- Chainlink `validFromTimestamp`
- Chainlink `observationsTimestamp`
- Chainlink `fullReport`
- Chainlink report hash
- source timestamp definition for `ts`
- whether `ts` is PRR publish time, PRR receive time, upstream observation time, or Chainlink report time
- whether prices are rounded, normalized, or resampled

If PRR can expose the signed Chainlink report or enough report metadata to prove `valid_from == window_open`, then PRR can be re-evaluated as a boundary source. Without that, PRR should remain reference-price input or comparator telemetry.

## Current Recommendation

Use both streams, but separate roles:

- Chainlink WS: primary signed-report stream, boundary-report cache, and preferred default reference source.
- Chainlink REST: deterministic boundary backfill and audit verifier.
- PRR WS: explicit continuous reference source is allowed for BTC/ETH/SOL/XRP, but not as automatic fastest-wins execution input.
- PRR for `price_to_beat`: no, unless PRR provides boundary-equivalent report semantics.
