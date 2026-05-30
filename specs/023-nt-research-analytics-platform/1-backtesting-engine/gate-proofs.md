# Backtesting Engine — Implementation Gate Proofs

Empirical results for the Implementation Gates in [`plan.md`](./plan.md). This is
the implementation-side proof record for issue **#438** (epic #437). It records
proof *results* against a concrete NautilusTrader revision; the cross-project
[`reference/evidence.md`](../reference/evidence.md) stays NT-version-agnostic and
**no evidence status is upgraded by this file**.

- **Branch:** `feat/438-bte-gate4-run-proof` (carries the Gate-1/2 proof scaffold
  forward from `feat/438-bte-gate1-backtest-proof`, which was not merged standalone).
- **Date:** 2026-05-30
- **NT revision proven:** `6e059dcbb59ac1e582132fc431a581936c216c3c` (v0.58.0) —
  the rev resolved by the target `bolt-v2` branch's `Cargo.toml`/`Cargo.lock`.
- **Proof artifact:** [`tests/bte_gate1_backtest_proof.rs`](../../../tests/bte_gate1_backtest_proof.rs)
  (gated behind the `bte-gate-proof` cargo feature).

## Status summary

| Gate | Task | Result | Notes |
|---|---|---|---|
| Gate 1 | BTE-001 | **PROVEN** | `nautilus-backtest` (+`streaming`) compiles in bolt-v2, pure Rust; `BacktestNode` constructs from a catalog-backed run config. |
| Gate 2 | BTE-007 | **PROVEN (local) + PROVEN (S3 interface); live-bucket round-trip deferred** | `ParquetDataCatalog` writes/reads instrument + trade + order-book-delta fixtures on local fs; `s3://` dispatches to the `cloud` object-store backend. |
| Gate 4 | BTE-029 | **PROVEN (strategy-less pipeline)** | `BacktestNode::run()` executes end-to-end over the catalog and emits a `BacktestResult`; results are pipeline-proof only (synthetic data, no `SourceProofReport`). |

All gates are exercised across the two spec market-structure fixtures (BTE-003)
plus additional NT instrument families enabled for capability/round-trip
coverage — **10 families across 9 distinct NT instrument types** (`9 of 18` of
NT's `InstrumentAny` catalogue):

| Family | NT instrument | Venue | Account | Strategy today? |
|--------|---------------|-------|---------|-----------------|
| binary-option | `BinaryOption` | POLYMARKET | Cash | ✅ bolt-v2 |
| cex-spot | `CurrencyPair` | BINANCE | Cash | ✅ bolt-v3 |
| cex-perp | `CryptoPerpetual` | BINANCE | Margin | ✅ bolt-v3 |
| perp-dex | `CryptoPerpetual` | HYPERLIQUID | Margin | ✅ bolt-v3 |
| equity-perp | `PerpetualContract` | REPRESENTATIVE | Margin | capability only |
| betting-betfair | `BettingInstrument` | BETFAIR | Cash | capability only |
| crypto-future | `CryptoFuture` | DERIBIT | Margin | capability only |
| crypto-option | `CryptoOption` | DERIBIT | Margin | capability only |
| crypto-futures-spread | `CryptoFuturesSpread` | DERIBIT | Margin | capability only |
| crypto-option-spread | `CryptoOptionSpread` | DERIBIT | Margin | capability only |

"Capability only" families are modeled, round-tripped, and run end-to-end to
prove NT can carry the family in our pipeline — **not** that bolt trades it. They
use representative (not venue-exact) economics: linear/USDC-settled shapes rather
than e.g. Deribit's inverse BTC settlement, which is a one-line TOML change. The
9 not yet covered are TradFi-only families (`Equity`, `Cfd`, `Commodity`,
`IndexInstrument`, `FuturesContract`/`FuturesSpread`,
`OptionContract`/`OptionSpread`) and `TokenizedAsset` — none has a bolt strategy
or data source today.

Every runtime value — venue, currencies, increments, balances, the synthetic
data points — is bound through the TOML registry
[`tests/fixtures/bte_market_families.toml`](../../../tests/fixtures/bte_market_families.toml),
deserialized with `serde`/`toml`. The proof holds **no** venue/price/currency
literals in Rust; the only structural choice it makes is which NT instrument
constructor a fixture's `kind` maps to. This implements the BTE-003 "venue/
provider selected only through TOML/registry bindings" requirement for the proof
fixtures (the production manifest's registry binding remains the #438 contract
slice).

## Reproduce

All commands run through the managed verifier (`scripts/rust_verification.py`):

```bash
# compile + lint the proof (Gate 1 compile evidence)
python3 scripts/rust_verification.py cargo --repo "$(git rev-parse --show-toplevel)" -- \
    clippy --features bte-gate-proof --test bte_gate1_backtest_proof -- -D warnings

# run the proofs (Gate 1 construct + Gate 2 local/S3 + Gate 4 run)
python3 scripts/rust_verification.py cargo --repo "$(git rev-parse --show-toplevel)" -- \
    test --features bte-gate-proof --test bte_gate1_backtest_proof -- --nocapture
```

Observed result (2026-05-30):

```text
running 11 tests
test binary_option_polymarket ... ok
test perps_spot_cex_spot_binance ... ok
test perps_spot_cex_perp_binance ... ok
test perps_spot_perp_dex_hyperliquid ... ok
test perpetual_contract_equity_perp ... ok
test betting_betfair_match_odds ... ok
test crypto_future_dated ... ok
test crypto_option_btc_call ... ok
test crypto_futures_spread_calendar ... ok
test crypto_option_spread_vertical ... ok
test gate2_s3_object_store_backend_is_wired ... ok
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
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
  singleton). For **each** of the ten market families the proof builds the
  realistic CLOB shape: an `L2_MBP` venue fed both `OrderBookDelta` and
  `TradeTick` data, with the correct account type (Cash for spot/binary/betting,
  Margin for perps/futures/options) and settlement currency.
- **Gate 2 (BTE-007), local.** For each family, `ParquetDataCatalog` writes the
  instrument (one of nine NT instrument types, via the dedicated
  `write_instruments`/`query_instruments` path that bypasses DataFusion) plus
  three `TradeTick`s and two `OrderBookDelta`s, and reads all three classes back
  byte-identical via `query_typed_data`. Local filesystem needs **no** cargo
  features (DataFusion + `object_store` are unconditional deps).
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
- **Gate 4 (BTE-029).** `BacktestNode::run()` runs each family's engine over the
  catalog and returns `Vec<BacktestResult>`. The Rust `BacktestEngineConfig`
  carries **no** strategies/actors (unlike the Python config — strategies are
  added imperatively via `engine.add_strategy`), so this is a strategy-less run:
  the engine iterates every catalog data point and emits a result with zero
  orders/positions. An `L2_MBP` venue enforces order-book data **at run time**
  too (not only at construction) — `run()` errors "No order book data found …
  when `book_type` is 'L2_MBP'" if the instrument has no deltas — which is why
  the proof writes a real (minimal) book. Observed per family: `iterations == 5`
  (3 trades + 2 deltas), `total_orders == 0`, `total_positions == 0`, populated
  `run_id` and `backtest_start`/`backtest_end`, and a settlement-currency PnL map
  at `0.0`. **Claim limit:** the data is synthetic with no `SourceProofReport`
  (BTE-015), so these results prove the *pipeline executes*, never market
  behaviour.
- **BTE-003 fixture binding.** The ten families are bound from the TOML registry
  `tests/fixtures/bte_market_families.toml` (serde/`toml`), proving the
  "venue/provider selected only through TOML/registry bindings" shape for the
  proof fixtures. A `kind` discriminant selects the NT constructor; no
  venue/price/currency literal lives in the Rust. Adding a family is a TOML edit
  plus (for a new `kind`) one constructor arm — never a hardcoded venue/price.

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
- **Strategy-driven run with fills** — the Gate-4 proof runs strategy-less (zero
  orders by design). A run that adds a strategy via `engine.add_strategy`,
  generates orders, and asserts fills/positions is later #438/#439 work, and
  needs real sourced data (`SourceProofReport`) to carry any market claim.
- **`SourceProofReport` and Artifact Index contracts** — BTE-005/015 (#438
  contract slice).

## Evidence rows advanced (no status change)

- **E-001** (SOURCE_PROVEN) — its `next_proof` "compile the NT version resolved by
  the target `bolt-v2` branch" is now empirically satisfied for the compile,
  config-construction, **and run-execution** portions (`BacktestNode::run()`
  returns a `BacktestResult`); "prove Bolt manifest maps to `BacktestRunConfig`/
  `BacktestDataConfig`" (BTE-009) remains open — the proof hand-builds the configs
  rather than deriving them from a `BacktestingRunManifest`.
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
