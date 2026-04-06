# NT-First Polymarket Data Lake V1

## Status

Draft design for review.

## Scope

This spec defines the minimum viable data capture and research architecture around the stock NautilusTrader live path used by `bolt-v2`.

It is intentionally narrow:

- Keep NautilusTrader (NT) as the live execution/runtime core.
- Reuse NT components wherever they already solve the problem.
- Build only the smallest custom layer needed for complete data retention and rigorous research.

This spec does **not** define alpha features, strategy logic, dashboards, or a full analytics platform.

## Upstream Baseline

- NautilusTrader GitHub: `https://github.com/nautechsystems/nautilus_trader`
- NT commit baseline for this design: `af2aefc24451ed5c51b94e64459421f1dd540bfb`
- Current local runtime path: [src/main.rs](/Users/spson/Projects/Claude/bolt-v2/src/main.rs:108)

## Design Principles

1. Use NT directly if it already provides the capability.
2. Add only thin glue when NT has the primitives but not the exact workflow.
3. Build custom only for capabilities NT does not provide.
4. Raw source payloads must be retained.
5. Normalized data must be reproducible from raw plus explicit lineage.
6. Research storage must optimize for S3 + Athena + DuckDB, not just NT-native internal layout.
7. The live trading node must not become the universal forever-capture process.

## NT-First Decision Matrix

### Use NT Directly

- Live runtime and strategy execution via `LiveNode`.
- Stock Polymarket data and execution adapters.
- NT normalized market-data vocabulary:
  - `QuoteTick`
  - `TradeTick`
  - `OrderBookDelta`
  - `InstrumentAny`
  - `InstrumentClose`
  - `InstrumentStatus`
- NT object-store and Parquet catalog primitives.
- NT backtest/replay engine.

### Thin Glue Around NT

- Instantiate and subscribe NT `FeatherWriter` on the message bus from the Rust live path.
- Configure NT object-store paths for S3 targets.
- Capture `InstrumentStatus` close/resolution events with a tiny companion subscriber.
- Batch-convert or materialize NT-native stream outputs into Athena/DuckDB-friendly Parquet tables.
- Use NT Gamma/instrument provider surfaces to populate a versioned market catalog.

### Build Custom

- Durable raw websocket payload capture.
- Durable raw HTTP response capture.
- Versioned market metadata history tables.
- Versioned resolution provenance/history tables.
- Trading-state lake export for orders, fills, and positions.
- Athena/Glue contracts and compaction policy.
- Research tables: features, labels, experiments.

## Reduced V1 Architecture

V1 uses only four components.

### 1. NT Live Node

Purpose:

- Run live Polymarket trading using the stock NT live path.

Responsibilities:

- Connect Polymarket data and execution clients.
- Normalize live venue traffic into NT domain events.
- Run strategies.

Non-goals:

- Raw payload archival.
- Research-table generation.
- Whole-venue capture.

### 2. Raw Capture

Purpose:

- Retain the original source payloads before NT-specific normalization choices matter.

Responsibilities:

- Record public websocket messages.
- Record private/user websocket messages if used.
- Record important HTTP responses used for market discovery and metadata.

Notes:

- This is one of the unavoidable custom components in V1.
- It should be transport-adjacent and dumb: capture bytes/payloads plus receive timestamps and request metadata.
- It must not perform normalized business logic.

### 3. NT Normalized Stream Sink

Purpose:

- Persist normalized market-data events with the least custom code possible.

Implementation:

- Use NT `FeatherWriter` as the default market-data sink.
- Wire it manually from the Rust live path by subscribing it to the message bus.
- Add a tiny companion subscriber for `InstrumentStatus` events, because stock FeatherWriter is market-data-centric.
- Default to local spool/output paths in V1 unless benchmarks prove direct remote object-store writes are safe for the live path.

Expected captured classes in V1:

- `quotes`
- `trades`
- `order_book_deltas`
- `order_book_depths`
- `index_prices`
- `mark_prices`
- `instrument_closes`
- `instruments`

Explicit V1 limitation:

- Stock NT FeatherWriter is market-data-centric and does not natively cover order/fill/position lake tables.
- Stock NT FeatherWriter does not cover `InstrumentStatus` close/resolution events.
- Those remain separate, deferred or custom-exported later.

### 4. Batch Lake + Enrichment Job

Purpose:

- Produce the canonical S3 Parquet lake used by Athena and DuckDB.

Responsibilities:

- Read raw captured payloads as needed.
- Read NT normalized stream outputs.
- Convert Feather stream outputs into canonical Parquet tables.
- Reshape NT-native stream layout into Athena/DuckDB-friendly lake layout.
- Enrich with market metadata and resolution history.
- Publish Athena-ready datasets.

Notes:

- In V1 this is one job family, not a fleet of microservices.
- It can run on a schedule.
- In practice this is ETL, not just compaction.

## Data Layers

Use only these three names:

- `raw`
- `normalized`
- `research`

### Raw

Meaning:

- Original source payloads, unchanged except for envelope metadata.

Purpose:

- Auditability.
- Re-parsing.
- Recovery from parser bugs.
- Provenance for research and replay.

### Normalized

Meaning:

- Stable, typed, queryable event and dimension tables.

Purpose:

- Replay.
- Backtests.
- Athena/DuckDB querying.
- Joining market behavior with metadata and outcomes.

### Research

Meaning:

- Derived feature, label, and experiment outputs.

Purpose:

- Alpha discovery and evaluation.

## Minimal V1 Tables

V1 intentionally uses a small set.

### Raw Tables

#### `raw_ws_messages`

Required columns:

- `stream_type`
- `channel`
- `market_id`
- `instrument_id` nullable
- `received_ts`
- `exchange_ts` nullable
- `payload_json`
- `source`
- `parser_version`
- `ingest_date`

#### `raw_http_responses`

Required columns:

- `endpoint`
- `request_params_json`
- `received_ts`
- `payload_json`
- `source`
- `parser_version`
- `ingest_date`

### Normalized Tables

#### `normalized_quotes`

Produced from NT market-data stream.

#### `normalized_trades`

Produced from NT market-data stream.

#### `normalized_order_book_deltas`

Produced from NT market-data stream.

#### `normalized_instruments`

Produced from NT market-data stream plus instrument discovery.

#### `normalized_market_closes`

Derived from custom-captured `InstrumentStatus` events with close/resolution semantics.

Clarification:

- This table is not the same thing as NT `InstrumentClose` market-data records.
- In Polymarket, resolution currently arrives through `InstrumentStatus` close events.

#### `normalized_markets`

Versioned market catalog dimension.

Required contents should include:

- stable market identity
- event/question text
- outcome names
- status
- timing metadata
- source observation timestamp

Primary source:

- NT Gamma/instrument provider surfaces, persisted with our own history/versioning.

#### `normalized_resolutions`

Versioned resolution fact table.

Required contents should include:

- stable market identity
- winning outcome
- observed resolution timestamp
- provenance/source
- any reason text available from NT or upstream metadata

V1 constraint:

- NT emits live close/resolution signals, but a complete history/backfill path may still require upstream polling outside the stock live node.

### Research Tables

Deferred from first cut of V1, but reserved:

- `research_features_v1`
- `research_labels_v1`
- `research_experiments`

## Canonical Identity

Use Polymarket stable identity as the backbone.

Canonical key:

- `condition_id + token_id`

Guidance:

- Treat NT `InstrumentId` as a representation that must map back to that pair.
- Do not use event slugs, titles, or outcome text as primary keys.
- Persist them as versioned descriptive attributes.

## Lineage Requirements

To make normalized outputs reproducible, store lineage fields with each normalized dataset build:

- `nt_commit`
- `adapter_config_hash`
- `normalizer_version`
- `raw_source_ref`
- `build_ts`

Rationale:

- NT can synthesize some outputs from venue data and configuration.
- Reproducibility requires more than raw payload retention.

## S3 Layout

Use a lake layout optimized for analytics, not NT’s internal catalog naming alone.

Top-level:

- `s3://<bucket>/raw/...`
- `s3://<bucket>/normalized/...`
- `s3://<bucket>/research/...`

Initial partition guidance:

- Raw: partition by `ingest_date`
- Normalized market-data tables: partition by `event_date`
- Market/resolution dimensions: partition by `observed_date`
- Research tables: partition by build date or feature/label version

Avoid over-partitioning in V1.

## Query Layer

### DuckDB

Primary use:

- Fast local iteration.
- One-off analysis.
- Feature prototyping.

### Athena

Primary use:

- Shared SQL.
- Reproducible research queries.
- Durable views and table contracts over S3 Parquet.

Design note:

- Athena is acceptable in V1, but only if file-count and partition sprawl are controlled.

## Failure Handling

### Live Node

- Must continue prioritizing trading/runtime correctness.
- Storage/export failures must not take down the trading loop.

### Raw Capture

- Must prefer append-only behavior.
- Dropped capture must be detectable via metrics/logging.

### Normalized Sink

- Can be restarted independently of strategy logic.
- Loss of normalized stream capture must not imply loss of raw capture.
- Treat direct remote object-store writes as opt-in after performance validation; local-first spool is the safe default.

### Batch Lake Job

- Can be rerun idempotently from raw and normalized inputs.

## Deferred Items

The following are explicitly out of V1:

- `normalized_orders`
- `normalized_fills`
- `normalized_positions`
- feature pipelines
- label pipelines
- experiment tracking UI
- advanced compaction/lakehouse upgrades
- whole-venue capture optimizations

These are deferred to keep V1 thin and NT-first.

## Build-Now Checklist

1. Keep stock NT live path unchanged for execution/runtime.
2. Add raw payload capture.
3. Wire NT FeatherWriter for normalized market-data capture.
4. Add a tiny `InstrumentStatus` subscriber for market close/resolution events.
5. Define canonical S3 lake paths and table names.
6. Build one batch ETL job to convert Feather outputs to canonical Parquet and enrich metadata/resolution history.
7. Validate Athena and DuckDB against those canonical tables.

## Explicit Non-Goals

Do not build:

- a new execution engine
- a new venue adapter
- a custom normalized market-data writer before trying NT FeatherWriter
- a custom Parquet/query engine
- a replacement runtime based on an OSS fork or unified API wrapper

## Open Questions For External Review

1. Is the reduced V1 too narrow, specifically around deferring `normalized_orders` / `fills` / `positions`?
2. Should V1 treat `normalized_market_closes` and `normalized_resolutions` as separate tables or one table?
3. Is Feather-first normalized capture the right bias, or should Parquet be written directly in V1 despite the extra custom work?
4. Is `condition_id + token_id` sufficient as the single canonical join identity across raw, normalized, and research layers?
