# Backtesting Engine — Implementation Gate Proofs

Empirical results for the Implementation Gates in [`plan.md`](./plan.md). This is
the implementation-side proof record for issue **#438** (epic #437). It records
proof *results* against a concrete NautilusTrader revision; the cross-project
[`reference/evidence.md`](../reference/evidence.md) stays NT-version-agnostic and
**no evidence status is upgraded by this file**.

- **Branch:** `feat/438-bte-gate1-backtest-proof`
- **Date:** 2026-05-30
- **NT revision proven:** `6e059dcbb59ac1e582132fc431a581936c216c3c` (v0.58.0) —
  the rev resolved by the target `bolt-v2` branch's `Cargo.toml`/`Cargo.lock`.
- **Proof artifact:** [`tests/bte_gate1_backtest_proof.rs`](../../../tests/bte_gate1_backtest_proof.rs)
  (gated behind the `bte-gate-proof` cargo feature).

## Status summary

| Gate | Task | Result | Notes |
|---|---|---|---|
| Gate 1 | BTE-001 | **PROVEN** | `nautilus-backtest` (+`streaming`) compiles in bolt-v2, pure Rust; `BacktestNode` constructs from a catalog-backed run config. |
| Gate 2 | BTE-007 | **PROVEN (local) + PROVEN (S3 interface); live-bucket round-trip deferred** | `ParquetDataCatalog` writes/reads instrument + trade fixtures on local fs; `s3://` dispatches to the `cloud` object-store backend. |

Both gates are exercised across **both** spec market-structure fixtures (BTE-003)
and four market families:

| Family | Fixture | NT instrument | Venue (example) | Account |
|--------|---------|---------------|-----------------|---------|
| binary option | `binary option` | `BinaryOption` | POLYMARKET | Cash |
| CEX spot | `perps/spot` | `CurrencyPair` | BINANCE | Cash |
| CEX perp | `perps/spot` | `CryptoPerpetual` | BINANCE | Margin |
| perp DEX | `perps/spot` | `CryptoPerpetual` | HYPERLIQUID | Margin |

Venue/currency are config/fixture parameters only — no hardcoded venue branch in
engine logic. (BTE-003 defines these fixtures via TOML/registry bindings; that
binding layer is the #438 contract slice — here the fixture *shapes* are proven
to round-trip and drive the engine.)

## Reproduce

All commands run through the managed verifier (`scripts/rust_verification.py`):

```bash
# compile + lint the proof (Gate 1 compile evidence)
python3 scripts/rust_verification.py cargo --repo "$(git rev-parse --show-toplevel)" -- \
    clippy --features bte-gate-proof --test bte_gate1_backtest_proof -- -D warnings

# run the three proofs (Gate 1 construct + Gate 2 local/S3)
python3 scripts/rust_verification.py cargo --repo "$(git rev-parse --show-toplevel)" -- \
    test --features bte-gate-proof --test bte_gate1_backtest_proof -- --nocapture
```

Observed result (2026-05-30):

```text
running 5 tests
test binary_option_polymarket ... ok
test perps_spot_cex_spot_binance ... ok
test perps_spot_cex_perp_binance ... ok
test perps_spot_perp_dex_hyperliquid ... ok
test gate2_s3_object_store_backend_is_wired ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

`clippy --features bte-gate-proof ... -D warnings` finished clean, and default
`cargo clippy --locked -- -D warnings` (no feature) finished clean without
compiling `nautilus-backtest` — confirming the proof does not enter the
production `LiveNode` build.

## What was proven (and the API facts behind it)

- **Gate 1 (BTE-001).** `nautilus-backtest` compiles against the pinned rev with
  `default-features = false, features = ["streaming"]`. `streaming` is mandatory:
  `BacktestNode` and the catalog-driven API are `#[cfg(feature = "streaming")]`
  (`backtest/src/lib.rs`). The pure-Rust path needs **no** `python`/pyo3 feature.
  `BacktestNode::new` performs real cross-validation — an `L2_MBP`/`L3_MBO` venue
  must have an order-book data config (`backtest/src/node.rs:341-368`), and only
  one run config is allowed per node (kernel `MessageBus` is a thread-local
  singleton). For **each** of the four market families the proof builds the
  realistic CLOB shape: an `L2_MBP` venue fed both `OrderBookDelta` and
  `TradeTick` data, with the correct account type (Cash for spot/binary, Margin
  for perps) and settlement currency.
- **Gate 2 (BTE-007), local.** For each family, `ParquetDataCatalog` writes the
  instrument (`BinaryOption` / `CurrencyPair` / `CryptoPerpetual`, via the
  dedicated `write_instruments`/`query_instruments` path that bypasses DataFusion)
  plus three `TradeTick`s, and reads both back byte-identical via
  `query_typed_data::<TradeTick>`. Local filesystem needs **no** cargo features
  (DataFusion + `object_store` are unconditional deps).
- **Gate 2 (BTE-007), S3 interface.** With the `cloud` feature on,
  `ParquetDataCatalog::from_uri("s3://…", None, …)` returns `Ok` (asserted via
  `is_ok()`), constructing the real S3 `object_store` backend instead of the
  "Cloud storage support requires the cloud feature" bail
  (`persistence/src/parquet.rs:539`). The builder is lazy — `object_store`'s
  `AmazonS3Builder::build` defaults the region to `us-east-1` and resolves the
  instance-credential provider at request time (`object_store-0.13.2`
  `aws/builder.rs:1086,1164`), so the construct succeeds without a network
  round-trip and the positive path is what the test asserts. S3 is hard-gated
  behind `cloud` (not in `default`); the `cloud` feature pulls **no** pyo3.

## Build isolation

`nautilus-backtest` is an **optional** dependency; the `bte-gate-proof` feature
(`Cargo.toml`) enables `dep:nautilus-backtest` + `nautilus-persistence/cloud`.
The feature is OFF by default, so the production binary never compiles
`nautilus-backtest` or the persistence S3 backend. This honours the package's
research-phase posture (live `LiveNode` untouched) while satisfying plan.md
Gate 1's "prove … compile" requirement on an explicitly authorised proof.

## Dependency footprint (proof-only)

Enabling `bte-gate-proof` adds **5** entries to `Cargo.lock` (verified
purely additive — `comm` against `main` shows zero removed/changed pins):

| Package | Version | Why |
|---------|---------|-----|
| `nautilus-backtest` | 0.58.0 | the gated dep itself |
| `md-5` | 0.10.6 | transitive (S3 request signing) |
| `quick-xml` | 0.39.4 | transitive (S3 XML responses) |
| `reqwest` | 0.12.28 | **second** `reqwest` — `object_store/cloud`'s HTTP client, alongside the existing `0.13.3` used by `alloy-transport-*`/`nautilus-network` |
| `wasm-streams` | 0.4.2 | **second** `wasm-streams` — transitive of `reqwest 0.12.28`, alongside the existing `0.5.0` |

The dual `reqwest`/`wasm-streams` versions are forced by upstream
(`object_store 0.13.2` pins the older `reqwest` line) and have **no production
impact**: `bte-gate-proof` is off by default, so the live `LiveNode` build links
neither. No existing pin moved — the live dependency graph is unchanged.

## Deferred (tracked, not done here)

- **Live-bucket S3 round-trip** (write/read against a real AWS bucket under a
  config-owned `artifact_root`). plan.md Gate 2 explicitly permits documenting the
  staging path instead; the interface is proven, the live round-trip lands with
  the #438 contract slice (E-034 schema/validation).
- **`BacktestNode::run()`** end-to-end over the catalog — Gate 4 / BTE-029.
- **`SourceProofReport` and Artifact Index contracts** — BTE-005/015 (#438
  contract slice).

## Evidence rows advanced (no status change)

- **E-001** (SOURCE_PROVEN) — its `next_proof` "compile the NT version resolved by
  the target `bolt-v2` branch" is now empirically satisfied for the compile +
  config-construction portion; "prove Bolt manifest maps to `BacktestRunConfig`/
  `BacktestDataConfig`" (BTE-009) remains open.
- **E-034 / E-038** — `ParquetDataCatalog` storage feasibility on local fs and the
  S3 backend is confirmed; the `artifact_root` schema, typed-subpath rules, and
  Artifact Index commit semantics remain DECISION_NEEDED (#438 contract slice).

## Operational note (disk governance)

A full debug build of bolt-v2 plus the backtest test dependencies (DataFusion +
`object_store/aws`) is ~55 GiB, which exceeds the managed-verifier cache soft
limit (50 GiB, spec 014/025 disk governance). Consecutive managed builds are
preflight-refused until `cargo clean -p bolt-v2` reclaims the bolt-v2 artifacts
(the NT/DataFusion dep rlibs are retained, so the rebuild is fast). Flagged for
spec-014/#494 cache-limit tuning if backtest builds become routine.
