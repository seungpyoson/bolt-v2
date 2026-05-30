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
| Gate 2 | BTE-007 | **PROVEN (local) + PROVEN (S3 interface); live-bucket round-trip deferred** | `ParquetDataCatalog` writes/reads a binary-option fixture on local fs; `s3://` dispatches to the `cloud` object-store backend. |

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
running 3 tests
test gate1_backtest_node_constructs_from_catalog_config ... ok
test gate2_local_catalog_round_trip_binary_option ... ok
test gate2_s3_object_store_backend_is_wired ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
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
  singleton). The proof builds the realistic binary-option/CLOB shape: an
  `L2_MBP` `POLYMARKET` venue fed both `OrderBookDelta` and `TradeTick` data.
- **Gate 2 (BTE-007), local.** `ParquetDataCatalog` writes one `BinaryOption`
  instrument (via the dedicated `write_instruments`/`query_instruments` path that
  bypasses DataFusion) plus three `TradeTick`s, and reads both back byte-identical
  via `query_typed_data::<TradeTick>`. Local filesystem needs **no** cargo
  features (DataFusion + `object_store` are unconditional deps).
- **Gate 2 (BTE-007), S3 interface.** With the `cloud` feature on,
  `ParquetDataCatalog::from_uri("s3://…")` dispatches to the real S3
  `object_store` backend rather than the "Cloud storage support requires the
  cloud feature" bail (`persistence/src/parquet.rs:539`). S3 is hard-gated behind
  `cloud` (not in `default`); the `cloud` feature pulls **no** pyo3. The S3
  builder is lazy, so this proves wiring without a network round-trip.

## Build isolation

`nautilus-backtest` is an **optional** dependency; the `bte-gate-proof` feature
(`Cargo.toml`) enables `dep:nautilus-backtest` + `nautilus-persistence/cloud`.
The feature is OFF by default, so the production binary never compiles
`nautilus-backtest` or the persistence S3 backend. This honours the package's
research-phase posture (live `LiveNode` untouched) while satisfying plan.md
Gate 1's "prove … compile" requirement on an explicitly authorised proof.

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
