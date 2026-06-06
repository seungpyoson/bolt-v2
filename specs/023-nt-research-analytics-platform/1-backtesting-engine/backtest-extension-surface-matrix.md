# Backtest Extension Surface Matrix

Status: current branch evidence for BACKTESTING_ENGINE-010 and BACKTESTING_ENGINE-011.

Scope: `backtesting-vertical-slice` at pinned NautilusTrader revision `6e059dcbb59ac1e582132fc431a581936c216c3c`.

Classification vocabulary:

- `defaulted`: NT default is used and the current slice does not expose a TOML knob.
- `pass_through`: TOML maps directly into an NT config field.
- `custom_owned`: Bolt owns a boundary, but only outside NT trading truth or through an NT-compatible interface.
- `unsupported_for_now`: a request must fail before `BacktestNode` runs.

## Evidence

- The BTE crate enables `nautilus-backtest` with `streaming, examples` and `nautilus-persistence` with `cloud`.
- NT `BacktestEngineConfig` exposes engine kernel, cache, message bus, data, risk, execution, portfolio, streaming, and analysis settings.
- NT `BacktestVenueConfig` exposes primitive venue behavior plus complex model surfaces: per-instrument leverages, margin model, simulation modules, fill model, latency model, fee model, and settlement prices.
- NT `BacktestDataConfig` exposes catalog path, filesystem protocol, storage options, data type, instrument filters, time filters, query filters, bar config, and optimized file loading.
- NT `BacktestNode` constructs `SimulatedVenueConfig`, `ParquetDataCatalog`, loads catalog instruments, queries catalog data, sets settlement prices, and runs the NT engine.
- NT `ParquetDataCatalog` supports local and object-store-backed catalog read/write/query, including S3 when the `cloud` feature is enabled.
- Current BTE manifest has `serde(deny_unknown_fields)` on all manifest sections, so unmodeled surface requests fail at TOML parse time instead of becoming hidden defaults.

## Matrix

| Surface | Current classification | NT-owned evidence | Current BTE binding | Decision |
| --- | --- | --- | --- | --- |
| Engine | `defaulted` for `BacktestEngineConfig`; `unsupported_for_now` for explicit engine subconfig | NT exposes `BacktestEngineConfig` fields for environment, trader id, timeouts, cache, message bus, data, risk, execution, portfolio, streaming, logging, and analysis | `to_nt_run_config()` does not set `engine`, so NT defaults are used; result contracts record the resolved default evidence in claim limits | Do not add a Bolt engine. Add typed TOML only when each target field is mapped and defaults are recorded. |
| Venue simulation | `pass_through` for primitive behavior; `unsupported_for_now` for complex models | NT maps `BacktestVenueConfig` into `SimulatedVenueConfig` before engine construction | TOML maps venue, OMS, account, book, balances, routing, frozen account, order support flags, bar/trade execution, liquidity, cash borrowing, queue position, OTO mode, base currency, default leverage, and price protection. Complex NT model fields are declared as optional manifest placeholders and rejected with `UnsupportedNtSurface`. | Keep primitive fields generic and TOML-owned. Complex model fields stay unsupported until each has a real typed NT mapping. |
| Run | `pass_through` for id/start/end; `defaulted` for chunk/dispose/raise-exception | NT `BacktestRunConfig` owns run id, venues, data, engine, chunk size, exception behavior, disposal, start, and end | TOML maps `run_id`, venue list, data list, `start_time`, and `end_time`; result contracts record pass-through id/start/end and resolved defaults for chunk size, exception behavior, and disposal | Add chunk/disposal only with explicit schema and tests. |
| Catalog | `pass_through` for current TradeTick catalog input; `unsupported_for_now` for unmodeled query surfaces | NT `BacktestDataConfig` and `ParquetDataCatalog` own catalog path, protocol, storage options, filters, and typed query | TOML maps catalog path, protocol, generic/Rust storage option maps, `TradeTick`, and one instrument id | Add new NT data classes through typed manifest support, not stringly ad hoc branches. |
| Strategy | `custom_owned` registry boundary, then NT-owned execution | NT engine accepts compiled Rust strategy objects through `add_strategy` | TOML selects a registered compiled Rust strategy key and typed parameters; inline code, Python paths, and untracked blobs are rejected | Keep strategies compiled and registered. No notebook/runtime strategy code in BTE. |
| Actor/execution-algorithm | `unsupported_for_now` | NT has actor/algorithm config surfaces outside this slice | No manifest section exists | Future support must be registry-based and NT-compatible. |
| Risk | `defaulted`; explicit request `unsupported_for_now` | NT `BacktestEngineConfig` accepts `RiskEngineConfig` | No manifest engine/risk section exists | Do not bypass or customize risk without typed mapping and result-contract proof. |
| Portfolio | `defaulted`; explicit request `unsupported_for_now` | NT `BacktestEngineConfig` accepts `PortfolioConfig` | No manifest portfolio section exists | Leave NT default until a typed portfolio schema is justified. |
| Execution | `defaulted`; explicit request `unsupported_for_now` | NT `BacktestEngineConfig` accepts `ExecutionEngineConfig`; venue execution is NT simulated exchange | BTE does not implement an execution engine | No Bolt execution path. Future fields must pass through NT config. |
| Cache | `defaulted`; explicit request `unsupported_for_now` | NT `BacktestEngineConfig` accepts `CacheConfig` | No manifest cache section exists | Keep default unless persistence/cache behavior becomes a proven requirement. |
| Message bus | `defaulted`; explicit request `unsupported_for_now` | NT `BacktestEngineConfig` accepts `MessageBusConfig` | No manifest msgbus section exists | Keep default. Add only with typed proof. |
| Streaming | `defaulted` off; explicit request `unsupported_for_now` | NT `BacktestEngineConfig` accepts `StreamingConfig`; `BacktestRunConfig` accepts `chunk_size` | Current runs load the bounded accepted object, not broad streaming backfill | Streaming is a future bounded-backfill concern, not part of this vertical slice. |
| Fill | `defaulted` to NT fill model; explicit request `unsupported_for_now` | NT `BacktestVenueConfig` exposes `fill_model`, and `BacktestNode` passes it into `SimulatedVenueConfig` | TOML field exists as an unsupported placeholder and fails with `UnsupportedNtSurface` | Do not build custom fills. Add NT model selection only through typed config. |
| Fee | `defaulted` to NT fee model; explicit request `unsupported_for_now` | NT `BacktestVenueConfig` exposes `fee_model` | TOML field exists as an unsupported placeholder and fails with `UnsupportedNtSurface` | Do not build custom fees. Add NT model selection only through typed config. |
| Latency | `defaulted` to no latency model; explicit request `unsupported_for_now` | NT `BacktestVenueConfig` exposes `latency_model`; exchange applies insert/update/delete latencies when present | TOML field exists as an unsupported placeholder and fails with `UnsupportedNtSurface` | Add a typed static/registered NT latency model only after source proof supports latency claims. |
| Margin | `defaulted` to NT account/instrument behavior; explicit `margin_model` unsupported | NT `BacktestVenueConfig` exposes `margin_model`; exchange applies it to margin accounts | TOML maps `account_type` and `default_leverage`; `margin_model` exists as an unsupported placeholder and fails with `UnsupportedNtSurface` | Keep margin model unsupported until typed and claim-limited. |
| Leverage | `pass_through` for default leverage; `unsupported_for_now` for per-instrument map | NT exposes `default_leverage` and `leverages` | TOML maps positive decimal `default_leverage`; `leverages` exists as an unsupported placeholder and fails with `UnsupportedNtSurface` | Per-instrument leverage map needs typed instrument-keyed schema before support. |
| Queue | `pass_through` for boolean queue-position tracking | NT exposes `queue_position` and matching engine consumes it | TOML maps `queue_position` | Enabling this does not by itself authorize execution-quality queue claims; source proof must support them. |
| Liquidity | `pass_through` for liquidity consumption flag | NT exposes `liquidity_consumption` | TOML maps `liquidity_consumption` | Trade-replay fixture still carries non-execution-quality claim limits unless L2/L3 proof exists. |
| Settlement | `unsupported_for_now` | NT exposes `settlement_prices`; `BacktestNode` sets them after instrument loading | TOML field exists as an unsupported placeholder and fails with `UnsupportedNtSurface` | Add typed settlement prices only for expiring instruments with source proof. |
| Order behavior | `pass_through` for current primitive controls | NT exposes stop/GTD/contingent/reduce-only/bar/trade/market-ack/OTO/price-protection behavior | TOML maps those controls directly | Keep as NT pass-through. Do not implement order mechanics in Bolt strategies. |
| Artifact governance | `custom_owned` boundary | NT does not know Bolt source-proof/result-contract rules | Bolt validates source proof, converter identity, checkpoint, manifest, catalog metadata, and result contract around NT | Keep this boundary thin and outside simulation truth. |

## Fail-Fast Status

Current fail-fast behavior is typed for known NT venue model surfaces and schema-based for still-unmodeled sections:

- `deny_unknown_fields` rejects explicit requests for unmodeled top-level, strategy, venue, catalog, or artifact-store fields before NT config construction.
- The manifest schema explicitly declares unsupported venue model placeholders for `leverages`, `margin_model`, `modules`, `fill_model`, `latency_model`, `fee_model`, and `settlement_prices`.
- Tests cover both empty and populated unsupported venue model requests and require `ManifestError::UnsupportedNtSurface` before NT config construction.
- Tests now cover an unsupported engine section request before NT config construction.
- Existing tests cover unsupported catalog data type, unsupported catalog filesystem protocol, shadowed catalog storage options, unsupported S3 option keys, invalid OMS/OTO/base-currency/leverage values, and rejected strategy sources.

This is intentional for the current slice: unsupported surfaces are not accepted as inert TOML, and they are not silently ignored by NT defaults. Future support must replace the structured rejection with a single typed NT pass-through path, not a second config path or venue/data-family constants.

## Venue And Data-Type Extension Rule

Adding a venue or data family must not add branches to the operator, runner, result contract, catalog execution, or NT invocation path.

For a compatible native-trade CSV source, the expected extension is:

1. Add or update an accepted `SourceProofReport`.
2. Add source/run-spec TOML with venue, instrument, source proof, and `[converter.csv]` column/time/side mapping.
3. Reuse the generic `csv-native-trades-to-canonical-trades.v1` adapter.
4. Let NT own `ParquetDataCatalog`, `BacktestDataConfig`, `BacktestVenueConfig`, and `BacktestNode`.

The current source-binding registry exercises this rule with two configured
backfillable native-trades bindings: the accepted Bybit sample binding and a
Binance spot native-trades candidate binding. The Binance row is registry data
only; it cannot become BTE input without the normal accepted source-proof,
sample, hash, and NT mapping gates.

For a non-compatible raw data shape, the expected extension is:

1. Add a new registered raw-source-to-canonical adapter with tests and converter config hash binding.
2. Keep venue/source values in TOML/source proof data, not production Rust constants.
3. Prove the output maps to an NT data class before it can become catalog/backtest input.
