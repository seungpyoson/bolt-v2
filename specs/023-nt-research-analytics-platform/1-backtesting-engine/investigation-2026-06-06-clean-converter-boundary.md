# Backtesting Engine Investigation And Fix Report - 2026-06-06

## Status

This is not a completed production backtesting-engine rollout. It is a completed fix for the converter/output-boundary slice plus explicit primitive NT venue-control and catalog cloud-config mapping.

Go for the local BNBUSDC vertical-slice path after this fix:

- accepted source proof -> canonical trade table -> NT `ParquetDataCatalog` projection -> NT `BacktestNode` run -> objective result contract
- dirty or mismatched converted outputs are rejected before catalog cleanup
- completed conversion writes durable `conversion-checkpoint.json`, `conversion-manifest.json`, and `catalog-metadata.json`
- result contracts bind the source object, converter identity, conversion manifest hash, conversion checkpoint hash, and catalog hash
- converter identity/version are declared in the run-spec TOML and validated against the compiled converter before any converted output is reused
- primitive NT `BacktestVenueConfig` controls are declared in TOML and mapped into NT rather than hidden behind NT defaults
- NT `BacktestDataConfig` catalog filesystem protocol and storage options are declared in TOML and mapped into NT, so S3/cloud catalog consumption can use NT's own catalog path
- the CLI has an explicit `--publish-output` opt-in that copies the verified local artifact tree to `manifest.output_prefix` through NT/object-store plumbing after the local run succeeds

No-go for broader production claims:

- `s3://bolt-parquet/nt-research-analytics/` currently has no clean output artifacts: `Total Objects: 0`, `Total Size: 0`
- the user confirmed converted artifacts were intentionally deleted during this investigation, so the empty prefix is expected current state, not accepted output evidence
- no real S3 write has been performed through `--publish-output` in this branch without explicit operator approval
- only the BNBUSDC 2026-03-01 trade-replay object is proven in this slice
- complex NT model surfaces are not yet manifest-configurable: leverage maps, margin model, simulation modules, fill model, latency model, fee model, and settlement prices
- no execution-quality, queue-position, order-book-liquidity, multi-day, or multi-instrument claim is supported by this slice

## Current Source Facts

- Accepted raw object exists: `s3://bolt-parquet/backfill-staging/2026-06-01/bybit/raw/v1/source=public_archive/family=tick_trades/category=spot/dt=2026-03-01/symbol=BNBUSDC/object=d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598.csv.gz`
- S3 listing result for that object: one object, `8505` bytes
- Accepted source proof is committed at `specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.bnbusdc-2026-03-01.json`
- Run spec binds the same object, hash, output prefix, and trade-replay claim limits in `specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml`
- Run spec also binds `[converter] identity = "bybit-public-archive-spot-tick-trades-to-canonical-trades.v1"` and `version = "1"`
- Clean NT output prefix is empty as of this investigation: `aws s3 ls s3://bolt-parquet/nt-research-analytics/ --summarize --recursive` returned `Total Objects: 0`, `Total Size: 0`
- Converted artifacts were intentionally deleted by the user to avoid confusing stale partial outputs with accepted clean outputs
- Local rebuilt-binary run against the accepted raw object wrote `/tmp/bte-real-e2e.RcFHhT/out` and produced `937` canonical rows, `937` NT catalog read-back trade ticks, NT `BacktestNode` iterations `937`, and catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`
- Local rerun into the same output directory kept the conversion manifest hash `b515ed12658f816e280b6f98d6a2fec52d1b28a6ff567ca4400196ac3c760272`, conversion checkpoint hash `0931a7a524219ddff66b6bde5dfaacc4eb884d7ae4555a557f48df037b7c0804`, catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`, and row count `937`
- Fresh release-binary local CLI run against the accepted raw object wrote `/tmp/bte-real-cli.sDvExP/out-local` and produced `937` canonical rows, `937` NT catalog read-back trade ticks, NT `BacktestNode` iterations `937`, and catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`
- Fresh release-binary `--publish-output` run against the accepted raw object and a local `file://` output prefix published 8 artifacts; `diff -qr /tmp/bte-real-cli.sDvExP/out-publish /tmp/bte-real-cli.sDvExP/publish-root/backtests/backtesting-vertical-slice-bnbusdc-2026-03-01` returned no differences

## NT Use Matrix

| Surface | NT capability | Bolt current use | Status |
| --- | --- | --- | --- |
| Instrument model | `CurrencyPair::new_checked`, `Price`, `Quantity`, `Money`, `InstrumentId`, `Symbol` | `catalog_projection.rs` builds the NT instrument from accepted instrument metadata, using checked constructors and NT precision | Uses NT |
| Market data model | `TradeTick` | canonical trade rows are converted into NT `TradeTick` values | Uses NT |
| Catalog writer | `ParquetDataCatalog::write_instruments`, `write_to_parquet` | catalog projection writes instrument and trade ticks through NT | Uses NT |
| Catalog reader | `ParquetDataCatalog::query_typed_data` / typed query path | read-back proof loads NT `TradeTick`s and compares against accepted rows | Uses NT |
| Backtest execution | `BacktestNode` + `BacktestRunConfig` | runner builds an NT run config, builds the node, injects the manifest strategy, and runs NT | Uses NT |
| Data loading into engine | `BacktestNode` creates `ParquetDataCatalog` from `BacktestDataConfig` and dispatches by `NautilusDataType` | manifest maps catalog input to `TradeTick`; engine consumption is checked by NT iteration count | Uses NT |
| Catalog cloud configuration | `BacktestDataConfig` supports `catalog_fs_protocol`, `catalog_fs_storage_options`, and `catalog_fs_rust_storage_options`; `BacktestNode` passes them to `ParquetDataCatalog::from_uri` | manifest now declares these fields and maps them into NT. Local operator mode resets them to `NONE` while binding a local projection root | Uses NT config surface; production S3 execution still pending |
| Venue simulation controls | `BacktestVenueConfig` supports routing, frozen account, stop/GTD/contingent controls, bar/trade execution, liquidity consumption, queue position, OTO trigger mode, base currency, default leverage, price boundary, leverage maps, margin model, simulation modules, fill model, latency model, fee model, and settlement prices | manifest maps primitive controls directly into NT: venue name, OMS type, account type, book type, starting balances, routing, frozen account, stop/GTD/contingent toggles, position/random/reduce-only toggles, bar/trade execution toggles, market-order ack toggle, liquidity consumption, cash borrowing, queue position, OTO trigger mode, base currency, default leverage, and price protection points | Uses NT for primitive controls; complex model surfaces pending |
| Converter checkpoint/idempotency | NT skips existing parquet files for identical target intervals, but does not provide Bolt source-proof/converter identity checkpointing | Bolt now owns a thin boundary wrapper that validates source proof, converter identity, checkpoint, manifest, and catalog metadata before rerun/resume | Correct Bolt-owned boundary |
| Source authorization/provenance | NT catalog APIs do not decide whether a raw source is accepted for this project | Bolt owns source proof acceptance and result-contract provenance binding | Correct Bolt-owned boundary |

Conclusion: we are not building a custom backtest engine, fill simulator, catalog format, or NT data loader. We are using NT for those. Bolt is building the governance boundary around NT because NT does not know this project source-proof contract or artifact provenance rules.

## Implemented Fix

Added `crates/backtesting-vertical-slice/src/conversion_boundary.rs`:

- `inspect_conversion_output` rejects non-empty output with no validated checkpoint
- mismatched source proof, object hash, converter identity, or converter version is rejected
- partial output with a valid non-completed checkpoint is resumable
- completed output requires manifest, checkpoint, and catalog metadata
- completed output returns stable conversion manifest, checkpoint, and catalog hashes

Updated `operator.rs`:

- reads converter identity/version from the run spec and validates them before touching output
- computes the expected conversion fingerprint before output cleanup
- inspects the output directory before deleting any local NT catalog
- writes a started checkpoint before conversion
- writes completed conversion artifacts after the backtest result contract is written
- preserves dirty output evidence when the output directory is invalid
- publishes the verified local artifact tree to `manifest.output_prefix` only through the explicit publish entrypoint

Updated `runner.rs` and `result_contract.rs`:

- builds conversion checkpoint, manifest, and catalog metadata from the accepted dataset and projected NT catalog
- adds `converter_identity`, `converter_version`, `conversion_manifest_hash`, and `conversion_checkpoint_hash` to the result contract
- validates those fields as required

Updated `run_manifest.rs`:

- declares primitive NT venue controls in the TOML-backed `ManifestVenueConfig`
- parses and validates NT OTO trigger mode, base currency, default leverage, and price-protection fields
- maps supported controls directly into `BacktestVenueConfig::builder()`
- rejects malformed OTO mode, base currency, and non-positive default leverage before the NT run
- declares NT catalog filesystem protocol and storage option maps in `ManifestCatalogInput`
- maps catalog cloud fields into `BacktestDataConfig::builder()`
- rejects unsupported catalog filesystem protocols before the NT run

Updated committed reference artifact:

- `specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-result-contract.bnbusdc-2026-03-01.json` now includes converter identity/version and conversion artifact hashes
- `specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml` now declares primitive NT venue controls and catalog filesystem fields explicitly

Updated CLI:

- default run remains local-only
- `--publish-output` runs the local proof first, then copies every produced artifact under `output_dir` to the configured `manifest.output_prefix`
- local `file://` publish roots are created before opening NT's local object store; remote prefixes are not pre-created locally

## TDD Evidence

RED checks observed before implementation:

- `just bte-test --test backtesting_vertical_slice_conversion_boundary` failed on unresolved `backtesting_vertical_slice::conversion_boundary`
- `just bte-test operator::tests::run_from_run_spec_writes_conversion_artifacts_and_contract_binds_them` failed because `BacktestResultContract` lacked the conversion provenance fields
- `just bte-test venue_config_maps_explicit_nt_venue_controls` failed with `E0609` missing `ManifestVenueConfig` fields such as `routing`, `frozen_account`, and `reject_stop_orders`
- `just bte-test data_config_maps_catalog_cloud_options` failed with `E0609` missing `ManifestCatalogInput` fields for catalog filesystem protocol and storage options

GREEN checks after implementation:

- `just bte-test --test backtesting_vertical_slice_conversion_boundary`: 6 passed
- `just bte-test result_contract::tests::result_contract_binds_manifest_and_acceptance_provenance`: 1 passed
- `just bte-test operator::tests::run_from_run_spec_writes_conversion_artifacts_and_contract_binds_them`: 1 passed
- `just bte-test operator::tests::committed_result_contract_deserializes`: 1 passed
- `just bte-test operator::tests::run_from_run_spec_rejects_dirty_output_without_conversion_checkpoint`: 1 passed
- `just bte-test venue_config_maps_explicit_nt_venue_controls rejects_unsupported_oto_trigger_mode rejects_invalid_base_currency rejects_non_positive_default_leverage`: 4 passed
- `just bte-test data_config_maps_catalog_cloud_options rejects_unsupported_catalog_fs_protocol operator::tests::committed_run_spec_deserializes`: 3 passed
- `just bte-test run_from_run_spec_and_publish_copies_artifacts_to_configured_prefix`: 1 passed
- `just bte-test cli_publish_output_flag_is_explicit_opt_in run_from_run_spec_and_publish_copies_artifacts_to_configured_prefix`: 2 passed
- `just bte-fmt-check`: passed
- `just bte-test`: 157 passed, including 2 slow public API tests
- `just bte-clippy`: passed
- `just bte-build`: passed
- rebuilt binary local accepted-object run: exit 0, `937` canonical rows, `937` NT read-back ticks, `937` NT iterations, stable catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`
- rebuilt binary local `--publish-output` run to a `file://` prefix: exit 0, 8 published artifacts, published tree matched local output tree byte-for-byte

## Remaining Unknowns And Concrete Solutions

| Unknown | Why it matters | Solution path |
| --- | --- | --- |
| Clean production output proof under `nt-research-analytics/` | The prefix is empty, so there is no S3 proof of a clean catalog/result path | run the accepted BNBUSDC object through the operator into the configured prefix, upload checkpoint/manifest/metadata/catalog/contract, then verify S3 listing and hashes |
| Broader source proof coverage | This slice proves one Bybit spot trade-replay object only | accept additional source proofs and bind each to converter identity, object hash, output prefix, and result contract |
| Complex NT venue model policy | Primitive controls are now explicit, but leverage maps, margin model, simulation modules, fill model, latency model, fee model, and settlement prices are not yet manifest-configurable | add typed manifest sections for each NT model surface we intend to support; for each unsupported model, fail validation with an explicit unsupported-surface error rather than silently relying on hidden defaults |
| Direct S3 catalog execution proof | Local operator path overrides catalog path to a local output root before running NT; the manifest can express S3 catalog config and the CLI can publish artifacts, but `BacktestNode` has not yet been proven to consume the published S3 catalog | run `--publish-output` with operator approval, verify the S3 artifact hashes, then run an explicit S3-backed catalog-consumption proof using NT `BacktestDataConfig` cloud fields |
| Old partial output disposition | Old outputs must not be promoted as clean | keep old outputs marked partial/dirty; after clean replacement exists, retain or archive them as forensic evidence, but do not use them as accepted result artifacts |

## Recommendation

Proceed with the next implementation slice as production proof plus remaining NT-surface hardening, not custom simulator work:

1. With operator approval, run `--publish-output` for the accepted BNBUSDC object, verify S3 listing and artifact hashes under `nt-research-analytics/`, then add S3-backed operator verification that proves `BacktestNode` consumes that clean catalog using NT's `BacktestDataConfig` cloud catalog fields.
2. Add typed manifest/config coverage for NT leverage maps, margin model, simulation modules, fill model, latency model, fee model, and settlement prices.
3. Make every unsupported NT venue model surface fail validation with an explicit contract error.
4. Only then claim a clean production BTE artifact path.
