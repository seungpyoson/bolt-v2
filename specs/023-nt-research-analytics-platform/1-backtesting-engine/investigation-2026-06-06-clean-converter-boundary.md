# Backtesting Engine Investigation And Fix Report - 2026-06-06

## Status

This is not a completed production backtesting-engine rollout. It is a completed fix for the converter/output-boundary slice plus explicit primitive NT venue-control, catalog cloud-config mapping, typed unsupported NT data-query surface gating, TOML-owned raw-payload bounds, TOML-owned artifact-store config, SSM-backed artifact-store credential resolution, and source-proof claim-limit propagation into generated result contracts.

Go for the local BNBUSDC vertical-slice path after this fix:

- accepted source proof -> canonical trade table -> NT `ParquetDataCatalog` projection -> NT `BacktestNode` run -> objective result contract
- dirty or mismatched converted outputs are rejected before catalog cleanup
- completed conversion writes durable `conversion-checkpoint.json`, `conversion-manifest.json`, and `catalog-metadata.json`
- result contracts bind the source object, converter identity, converter config hash, conversion manifest hash, conversion checkpoint hash, catalog metadata hash, and catalog hash
- result contract `manifest_hash` binds the submitted portable run-spec manifest, not the operator's temporary local catalog path
- converter identity/version, raw payload container, object/decoded byte budgets, and CSV native-trade column mapping are declared in the run-spec TOML; the operator validates identity/version and container config against the registered converter and binds the full converter config hash before any converted output is reused
- CSV native-trade mapping now declares whether raw CSV payloads have headers; headerless sources use the accepted source-proof schema columns as the logical schema, so Binance Data Vision-style native trade ZIPs do not require venue-specific code
- the CLI and operator both fail fast when the local object byte count differs from `accepted_object.bytes` or exceeds `converter.raw_payload.max_object_bytes`; the CLI rejects the configured object-size policy before reading the payload into memory, and the operator rejects mismatches before hashing, decompression, checkpoint writes, or NT work
- gzip, plain CSV, and single-member ZIP payload decoding is bounded by `converter.raw_payload.max_decoded_bytes`, so compressed/archive expansion rejects before canonical artifact writes or NT catalog projection
- catalog metadata records both the portable output catalog URI and the actual execution catalog URI, plus whether direct S3 catalog access was proven
- primitive NT `BacktestVenueConfig` controls are declared in TOML and mapped into NT rather than hidden behind NT defaults
- NT `BacktestDataConfig` catalog filesystem protocol and storage options are declared in TOML and mapped into NT, so S3/cloud catalog consumption can use NT's own catalog path
- NT `BacktestDataConfig` query surfaces not yet supported by this slice (`instrument_ids`, data-config `start_time`/`end_time`, `filter_expr`, `client_id`, metadata, `bar_spec`, `bar_types`, and `optimize_file_loading`) are declared in TOML schema, rejected with structured `UnsupportedNtSurface` errors before NT config construction, and recorded as unsupported resolved NT surfaces for result-contract claim limits
- artifact-root typed subpaths now resolve from the single configured root for `raw`, `nt-catalog`, `source-proofs`, `backtests`, `artifact-index`, and `research-analytics`; unsupported artifact-root schemes fail validation before a run
- Artifact Index record construction now has a pure contract helper for BTE-produced staged records: generated per-kind event/latest-pointer URIs under the single S3 `artifact_root`, `sha256` content-hash validation, required parent lineage refs, active lifecycle default, and producer-owned write-authority checks
- Artifact Index committed-discovery helpers now validate that generated latest pointers resolve only to matching active snapshots, reject stale or hash-invalid pointers, reject staged/orphan records as committed discovery truth, and model first-write/update pointer preconditions plus retry/rebase-required conditional failures
- Artifact Index cross-kind parent resolution now requires the child record's manifest lineage id and `sha256` hash to match the parent record, rejecting independently supplied latest-parent records with mismatched hashes
- Artifact Index event-create planning now models producer-owned immutable events: same URI plus same structured payload hash is idempotent, while a different payload at the same URI is rejected as an overwrite
- the pinned `object_store = 0.13.2` API exposes `PutMode::Create` and `PutMode::Update(UpdateVersion)` for optimistic-concurrency metadata commits; its AWS backend maps the canonical `conditional_put = "etag"` option to `If-None-Match: *` create-only writes and `If-Match: <etag>` updates, and the manifest now preserves that option while rejecting `conditional_put = "disabled"` for S3 artifact-store commits
- Artifact lifecycle config now rejects default delete/expiration rules, requires `active`/`archive`/`deep_archive` storage profiles, and derives active-to-inactive state from a configured quiet window
- S3 catalog storage options now fail before NT config construction if generic and Rust-specific maps are both set, or if an S3 option key is not supported by this pinned NT revision
- source-proof acceptance now enforces the schema rule that accepted canonical backfill input must use `directly_backfillable` or `owner_archive_backfillable`; bounded/current-only, pending, vendor/forward-capture-only, not-applicable, or excluded evidence states cannot become accepted BTE input
- source-proof acceptance now cross-checks registered TOML source-binding metadata for `product_family`, `table_family`, and `evidence_state`; a proof cannot reuse a registered host/key while silently changing the data family or acceptance state
- source-proof acceptance now rejects unknown `source_binding`/venue pairs before an accepted proof can be stamped; object selection keeps a defense-in-depth rejection for forged accepted records
- source-proof acceptance now requires `raw_sample_uri` and `schema_sample_uri` to be staged `s3://` artifact URIs, and accepted dataset selection requires the manifest object's `s3_uri` to exactly match the proof's `raw_sample_uri`
- accepted dataset selection now validates the staged object's `source_url` against the registered source-binding URI template path/query, not just the HTTPS host; a same-host monthly, aggTrades, or other data-family path cannot satisfy a daily trades binding
- source-proof acceptance now requires structured `acceptance_scope` facts (`planned_objects`, `completed_objects`, `failed_objects`, `skipped_objects`, `accepted_bytes`, and `selector_scope_violations`) instead of accepting prose-only completeness evidence; failed objects, selector-scope violations, inconsistent object accounting, skipped objects without a gap policy, and selected objects whose bytes exceed accepted bytes fail before canonical conversion
- non-L2 source-proof acceptance now requires structured `claim_limits` rows backing every `forbidden_claims` entry, so trade-replay or weaker data cannot rely on unstructured prose to block execution-quality, order-book, coverage, or fidelity claims
- generated result contracts now preserve structured source-proof claim-limit evidence from the accepted proof instead of rebuilding source limits from plain `forbidden_claims` strings
- non-latest source-proof pins now require structured manifest justification: `normal` runs still cannot pin them, non-normal pins require `proof_pin_reason_code`, and `audit_or_investigation` pins require `proof_pin_reason_detail`
- the accepted `proof_pin_reason_code` vocabulary now matches the plan/reference contract, including published-result reproduction and regression-comparison pins
- the CLI has an explicit `--publish-output` opt-in that copies the verified local artifact tree to `manifest.output_prefix` through NT/object-store plumbing after the local run succeeds
- published artifacts are create-only: the operator preflights the bounded target artifact set and writes through object-store `PutMode::Create`, so an existing published artifact rejects the run instead of being overwritten
- publish flows resolve and validate artifact-store options before reading the accepted object, so missing S3/SSM setup cannot waste local object I/O on large accepted objects
- artifact-store options are TOML-owned; raw S3 credentials in TOML are rejected; `s3://` publish/proof requires `[manifest.artifact_store.ssm_parameters]` to resolve `access_key_id` and `secret_access_key` through the Rust AWS SDK before any backtest or object-store operation starts
- the current `BacktestExtensionSurface` classification is recorded in `backtest-extension-surface-matrix.md`; supported primitive NT controls are TOML pass-throughs, Bolt-owned pieces are provenance/governance boundaries, unmodeled NT model/system surfaces fail before NT config construction, and each successful run now writes `backtest-run-manifest.json` plus result-contract claim-limit entries for resolved NT defaults, supported run/venue/catalog pass-through fields, and unsupported NT surfaces

No-go for broader production claims:

- `s3://bolt-parquet/nt-research-analytics/` currently has no clean output artifacts: `Total Objects: 0`, `Total Size: 0`
- the user confirmed converted artifacts were intentionally deleted during this investigation, so the empty prefix is expected current state, not accepted output evidence
- no real S3 write has been performed through `--publish-output` in this branch without explicit operator approval and configured SSM artifact-store credential parameter paths
- non-secret SSM parameter-name searches for `artifact`, `s3`, and `backtest` returned no candidate parameter names, so the required `[manifest.artifact_store.ssm_parameters]` paths are not known in the current AWS account
- current operator runs still stamp `direct_s3_catalog_access_proven = false` because `BacktestNode` consumes the verified local projection root before optional publish
- the direct S3 publish/proof command with only `region` configured now fails fast with `artifact_store.ssm_parameters must resolve access_key_id and secret_access_key before publishing to an s3 output_prefix`, and leaves no output directory
- only the BNBUSDC 2026-03-01 trade-replay object is proven in this slice; Bybit is a sample source/proof, not a production converter special case
- generic `run_manifest` unit fixtures now use synthetic accepted dataset values rather than duplicating the accepted Bybit/BNBUSDC sample proof; the committed sample proof/run-spec and end-to-end sample fixtures remain the authoritative BNBUSDC evidence
- the registry now carries a second native-trades source-binding candidate (`binance-spot-native-trades`) so the binding gate is not single-venue, and the converter can process its headerless single-member ZIP shape; no Binance trade object is accepted BTE input until staged S3 raw/schema samples, source proof, instrument-universe proof, and sample/hash checks pass
- complex NT model surfaces are now manifest-declared but not mapped into NT: leverage maps, margin model, simulation modules, fill model, latency model, fee model, and settlement prices fail with structured `UnsupportedNtSurface` errors before NT config construction.
- compatible CSV native-trade spot venues can be added through source proof plus run-spec TOML mapping, including `csv_gzip`, `csv_text`, configured single-member `single_csv_zip` containers, headered or headerless schemas, timestamp units, and side-token mapping; new NT data classes or instrument families beyond the current `TradeTick`/`CurrencyPair` path are not yet TOML-only and must fail fast until a typed NT projection path is added
- no execution-quality, queue-position, order-book-liquidity, multi-day, or multi-instrument claim is supported by this slice

## Current Source Facts

- Current worktree recheck on 2026-06-07 after code commit `14fe2b6c10710173976c33ef51d8a902c3d52dd4`: branch `codex/bte-clean-converter-nt-use-continue`, `main = origin/main = 5f5cc430a3fddda20823629ee3c8e271bc54edb5`, and `git status --short` reported no dirty files before this report update.
- Relevant local/remote BTE branches visible from this worktree include `codex/backtesting-engine-increment`, `codex/bte-clean-converter-nt-use`, `codex/bte-e037-trace-row`, `chore/bte-runtime-hardening`, `feat/439-nt-venue-converters`, `codex/backtesting-vertical-slice`, `feat/438-bte-gate4-run-proof`, `feat/438-bte-ingest-loader`, and `feat/438-bte-gate1-backtest-proof`; stale branches are forensic references only and are not implementation sources for this branch.
- Open PR recheck with `gh pr list --search "BTE OR backtesting OR converter" --state open` returned draft PR `#592` (`codex/bte-e037-trace-row`), draft PR `#496` (`feat/438-bte-gate4-run-proof`), and open docs PR `#576` (`docs/438-normalization-catalog-plan`); none of these PRs proves this branch's clean production artifact path.
- Accepted raw object exists: `s3://bolt-parquet/backfill-staging/2026-06-01/bybit/raw/v1/source=public_archive/family=tick_trades/category=spot/dt=2026-03-01/symbol=BNBUSDC/object=d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598.csv.gz`
- S3 listing result for that object: one object, `8505` bytes
- Fresh read-only S3 `head-object` after the CLI preflight fix confirmed the accepted raw object still exists with `ContentLength = 8505` and ETag `"3959bd2c4ff9ac093c7692b812cea2f8"`; later, only this single approved 8505-byte object was downloaded to `/private/tmp/bte-bnbusdc-current-schema-object.csv.gz` for the current-schema local proof
- Accepted source proof is committed at `specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.bnbusdc-2026-03-01.json`
- Run spec binds the same object, hash, output prefix, and trade-replay claim limits in `specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml`
- Run spec now configures `[manifest.artifact_store].rust_storage_options = { region = "us-east-1", conditional_put = "etag" }` so the S3 artifact-store path is explicit about object_store conditional-write semantics instead of relying on a hidden default
- Run spec binds `[converter] identity = "csv-native-trades-to-canonical-trades.v1"`, `version = "1"`, `[converter.raw_payload] container = "csv_gzip"`, `max_object_bytes = 8505`, `max_decoded_bytes = 1048576`, and `[converter.csv] has_headers = true` plus column/timestamp/side-token mapping. The Bybit-specific values live in the sample source proof and run-spec data, not in operator/runner control flow.
- Source-binding registry coverage is no longer single-venue for native trades: `bybit-spot-tick-trades` and `binance-spot-native-trades` are both configured as backfillable `native-trades`/`trades` bindings. The Binance row points at Binance Data Vision spot daily `trades` zip files and remains a candidate only; it does not create an accepted proof or bypass object/sample/hash gates.
- Bounded Binance candidate evidence collected without broad backfill: `https://data.binance.vision/data/spot/daily/trades/BNBUSDC/BNBUSDC-trades-2026-03-01.zip` returned `200`, `content-length = 1066394`, one CSV member `BNBUSDC-trades-2026-03-01.csv`, decoded length `5287070`, row count `71431`, and ZIP SHA256 `433d32b8d828abee5e1937e01372d16f7edadc14c41fe736b0b9577541fa5e81`, matching the Binance `.CHECKSUM` sidecar. The CSV is headerless with columns matching Binance native-trades semantics (`trade_id`, `price`, `qty`, `quote_qty`, `time`, `is_buyer_maker`, `is_best_match`) and microsecond timestamps; it is evidence for adapter coverage only, not an accepted source proof.
- Read-only fetch of the existing one-off Binance staging manifest from `s3://bolt-parquet/backfill-staging/2026-06-01/binance/manifests/v1/run=binance-backfill-run-d928f6666827dd47/binance-backfill-manifest.json` produced `4701` payload records, `11600667` bytes, and SHA256 `b37d01f30932c8af4d8b9bc112b031569afbdf3a2db3756ec1f2a79626880c36`. It contains no `spot`/`daily`/`trades`/`BNBUSDC` object for `2026-03-01`; the staged BNBUSDC trade object is monthly March 2026 at `s3://bolt-parquet/backfill-staging/2026-06-01/binance/raw/v1/source=data.binance.vision/product=spot/frequency=monthly/family=trades/symbol=BNBUSDC/dt=2026-03/object=9fcdae9872ab3c7ff8f13d5f3c1830b017b25561fe6140493decfa079ee56aa6.zip`, so it cannot satisfy the configured daily source binding.
- Clean NT output prefix is empty as of this investigation: `aws s3 ls s3://bolt-parquet/nt-research-analytics/ --summarize --recursive` returned `Total Objects: 0`, `Total Size: 0`
- Fresh read-only S3 object-count query after the CLI preflight fix returned `0` objects under `s3://bolt-parquet/nt-research-analytics/`
- Fresh read-only recheck on 2026-06-07 returned no non-secret SSM parameter names containing `artifact`, `s3`, or `backtest`, and `aws s3 ls s3://bolt-parquet/nt-research-analytics/ --summarize --recursive` still reported `Total Objects: 0`, `Total Size: 0`
- Converted artifacts were intentionally deleted by the user to avoid confusing stale partial outputs with accepted clean outputs
- Local rebuilt-binary run against the accepted raw object wrote `/tmp/bte-real-e2e.RcFHhT/out` and produced `937` canonical rows, `937` NT catalog read-back trade ticks, NT `BacktestNode` iterations `937`, and catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`
- Earlier pre-raw-payload-schema local generic-converter CLI proof into `/private/tmp/bte-s3-proof/out-local-generic-converter` produced converter config hash `4e54ce1edbdab877a776cb5d38ede603a747da49c0355f80b2f3665905333080`, conversion manifest hash `7d6d48376c026174bb84830dc6058e4eddecf9e3632344431413a4b2b3ca8352`, conversion checkpoint hash `60429ebd758ec1b0383dbedd0d0e38997a0bc90f33f2dc2ba2bf5bf6b1bd5842`, catalog metadata hash `3b9ee2bd6980de74aa30b677a408f073d5f68ff6aaf81a338425a2924709e587`, catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`, row count `937`, and NT iterations `937`; after adding `[converter.raw_payload]` and byte bounds, these pre-schema hashes are forensic evidence only
- Fresh current-schema local CLI run against the accepted raw object wrote `/private/tmp/bte-bnbusdc-current-schema-out-20260607a` and produced `937` canonical rows, `937` NT catalog read-back trade ticks, NT `BacktestNode` iterations `937`, catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`, converter config hash `a20a83ef7bf42926e394f16c43c09819b58fc8c08d42ceb23352f9a61e293144`, conversion manifest hash `a35705f61ffc42c6fb019fa4e457e9d655d63825cd6401383c0977bafc951ccb`, conversion checkpoint hash `3c77e46b26b6998ce0edba532b3a608521490b2afb61e3a304e121a0b74ae0e5`, and local catalog metadata hash `e552c285d2fb52521a38add9fbe94af360a5eb88e34dd63e8edca25f19a0a0e9`; the committed portable reference catalog metadata hash is `f82bd70268d1df4163c1746ad79194fc987082e4b6ab9cdc82d6d8275990e882` because it uses reference URIs instead of the local `/private/tmp` execution URI
- Fresh release-binary local CLI run against the accepted raw object wrote `/tmp/bte-real-cli.sDvExP/out-local` and produced `937` canonical rows, `937` NT catalog read-back trade ticks, NT `BacktestNode` iterations `937`, and catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`
- Fresh release-binary `--publish-output` run against the accepted raw object and a local `file://` output prefix published 8 artifacts; `diff -qr /tmp/bte-real-cli.sDvExP/out-publish /tmp/bte-real-cli.sDvExP/publish-root/backtests/backtesting-vertical-slice-bnbusdc-2026-03-01` returned no differences
- Fresh current-branch local CLI run after SSM artifact-store changes wrote `/private/tmp/bte-s3-proof/out-local-after-ssm-artifact-store-3` and produced `937` canonical rows, `937` NT catalog read-back trade ticks, NT `BacktestNode` iterations `937`, and catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`
- Fresh current-branch S3 publish/proof command with only `region` configured failed before the backtest with `artifact_store.ssm_parameters must resolve access_key_id and secret_access_key before publishing to an s3 output_prefix`; `/private/tmp/bte-s3-proof/out-s3-missing-ssm-fail-fast` was not created
- Non-secret SSM parameter-name searches for `artifact`, `backtest`, `s3`, `parquet`, `research`, `nt`, and `credential` returned no names; a broad `bolt` name search returned only unrelated Chainlink/testnet and one-off backfill entries, so direct real S3 proof cannot be completed until valid SSM parameter paths are provided or created outside this branch's code path

## NT Use Matrix

Pinned NT revision evidence: `Cargo.toml:25-46` and
`crates/backtesting-vertical-slice/Cargo.toml:39-43` pin NautilusTrader at
`6e059dcbb59ac1e582132fc431a581936c216c3c`; local evidence below is from the
Cargo git checkout for that revision.

## Prompt NT-Use Matrix

| Requirement | NT provides? | Exact NT / BTE evidence | Decision |
| --- | --- | --- | --- |
| converter raw archive -> NT data | No for this project's raw-source proof and CSV archive normalization; yes once rows are NT typed data | NT `ParquetDataCatalog::write_to_parquet<T>` requires typed records implementing NT persistence traits, not project source-proof acceptance, object-container decoding, or CSV column mapping (`crates/persistence/src/backend/catalog.rs:505-575`). BTE owns the generic CSV native-trade mapping, TOML-selected raw payload container, and registered converter (`crates/backtesting-vertical-slice/src/canonical_trades.rs`, `crates/backtesting-vertical-slice/src/operator.rs`). | Bolt owns only the raw-source-to-canonical adapter and source-proof gate; venue/source/container values stay in TOML/source proof data. |
| catalog write/read | Yes | NT `ParquetDataCatalog` is object-store-backed and typed (`crates/persistence/src/backend/catalog.rs:154-165`), supports local/S3/cloud URI construction (`crates/persistence/src/backend/catalog.rs:213-325`), writes typed Parquet (`crates/persistence/src/backend/catalog.rs:505-575`), and reads typed data (`crates/persistence/src/backend/catalog.rs:1732-1754`). BTE uses those APIs in `catalog_projection.rs`. | Use NT catalog; do not build a custom catalog format. |
| backtest execution | Yes | NT `BacktestNode` connects `ParquetDataCatalog` and `BacktestEngine` (`crates/backtest/src/node.rs:42-69`), and `BacktestRunConfig` owns venues/data/engine/run windows (`crates/backtest/src/config.rs:795-823`). BTE calls `BacktestNode::new`, `build`, and `run` in `runner.rs`. | Use NT `BacktestNode`; do not build a Bolt simulator. |
| BacktestDataConfig / catalog cloud path | Yes, but real S3 execution proof is pending | NT `BacktestDataConfig` carries `catalog_path`, `catalog_fs_protocol`, generic storage options, and Rust storage options (`crates/backtest/src/config.rs:595-725`). `BacktestNode` builds `protocol://catalog_path`, chooses Rust options over generic options, and calls `ParquetDataCatalog::from_uri` (`crates/backtest/src/node.rs:503-512`). `nautilus-persistence` `cloud` enables S3/Azure/GCP/HTTP object stores (`crates/persistence/src/lib.rs:36`). | Use NT cloud catalog config; keep production S3 proof no-go until valid SSM-backed artifact-store options are configured and `--prove-published-catalog` runs. |
| NT catalog data classes | NT supports more than current BTE exposes | NT dispatches `QuoteTick`, `TradeTick`, `Bar`, `OrderBookDelta`, `OrderBookDepth10`, `MarkPriceUpdate`, `IndexPriceUpdate`, `InstrumentStatus`, and `InstrumentClose` from `BacktestDataConfig` (`crates/backtest/src/node.rs:526-568`). Current BTE manifest maps only `"TradeTick"` and rejects other strings (`crates/backtesting-vertical-slice/src/run_manifest.rs:736-745`). | Current slice is intentionally `TradeTick` only. Add new NT data classes through typed manifest/projection support, not stringly venue branches. |
| BacktestVenueConfig / venue simulation controls | Yes | NT `BacktestVenueConfig` exposes primitive controls and complex model surfaces: routing, frozen account, order flags, bar/trade execution, liquidity, queue, OTO mode, base currency, default/per-instrument leverage, margin model, modules, fill/latency/fee models, price protection, and settlement prices (`crates/backtest/src/config.rs:331-389`). Current BTE maps primitive controls and declares unsupported complex model fields, rejecting them with `UnsupportedNtSurface` before NT config construction. | Use NT venue config. Keep complex models `unsupported_for_now` until each has a real typed, claim-limited NT mapping. |
| resume/checkpoint/idempotency | Partially: NT skips existing exact Parquet file and enforces disjoint intervals; NT does not know Bolt source-proof/converter checkpoint semantics | NT `write_to_parquet` returns without writing if the object already exists and checks interval disjointness before writing (`crates/persistence/src/backend/catalog.rs:537-556`). It does not bind source proof id/version, raw object hash, converter identity/version/config hash, conversion manifest hash, or checkpoint state. | Bolt owns a thin conversion boundary around NT output; do not rely on NT file-skip behavior as acceptance/idempotency proof. |
| artifact/proof governance | No | NT has no concept of Bolt `SourceProofReport`, accepted-object byte/hash gate, artifact-root typed subpaths, proof-pin policy, or objective `BacktestResultContract`. Current BTE owns those in `source_proof.rs`, `conversion_boundary.rs`, `run_manifest.rs`, and `result_contract.rs`. | Bolt-owned governance is necessary and must stay outside simulation/execution truth. |

Detailed extension-surface classification is recorded separately in `backtest-extension-surface-matrix.md`.

| Surface | NT capability | Bolt current use | Status |
| --- | --- | --- | --- |
| Instrument model | `CurrencyPair::new_checked`, `Price`, `Quantity`, `Money`, `InstrumentId`, `Symbol` | `catalog_projection.rs` builds the NT instrument from accepted instrument metadata, using checked constructors and NT precision | Uses NT |
| Market data model | `TradeTick` | canonical trade rows are converted into NT `TradeTick` values | Uses NT |
| Raw source -> canonical trades | NT does not parse this project's accepted raw archive/proof contract into project canonical rows | Bolt owns a generic `csv-native-trades-to-canonical-trades.v1` adapter driven by `[converter.raw_payload]` object-container config plus `[converter.csv]` TOML column, timestamp-unit, and side-token mapping; source/venue values remain in source proof and run-spec data | Correct Bolt-owned adapter; no operator/runner venue hardcode |
| Catalog writer | `ParquetDataCatalog::write_instruments`, `write_to_parquet` | catalog projection writes instrument and trade ticks through NT | Uses NT |
| Catalog reader | `ParquetDataCatalog::query_typed_data` / typed query path | read-back proof loads NT `TradeTick`s and compares against accepted rows | Uses NT |
| Backtest execution | `BacktestNode` + `BacktestRunConfig` | runner builds an NT run config, builds the node, injects the manifest strategy, and runs NT | Uses NT |
| Data loading into engine | `BacktestNode` creates `ParquetDataCatalog` from `BacktestDataConfig` and dispatches by `NautilusDataType` | manifest maps catalog input to `TradeTick`; engine consumption is checked by NT iteration count | Uses NT |
| Catalog cloud configuration | `BacktestDataConfig` supports `catalog_fs_protocol`, `catalog_fs_storage_options`, and `catalog_fs_rust_storage_options`; `BacktestNode` passes them to `ParquetDataCatalog::from_uri` | manifest now declares these fields and maps them into NT. Local operator mode resets them to `NONE` while binding a local projection root | Uses NT config surface; production S3 execution still pending |
| Artifact-store cloud options | NT/object-store accepts explicit S3 options such as `region`, `access_key_id`, `secret_access_key`, `session_token`, and `conditional_put`; the pinned NT S3 path does not read AWS shared-credentials files because it constructs `AmazonS3Builder::new()` and applies only explicit options | manifest now declares artifact-store storage options and SSM parameter paths. The CLI resolves SSM through the Rust AWS SDK before publish/proof and passes the resolved object-store options into both artifact publish and published-catalog proof. For Artifact Index commit readiness, the S3 artifact-store path preserves `conditional_put = "etag"` and rejects `disabled` | Uses NT/object-store explicit option surface; Bolt-owned SSM wrapper is required by repo secret policy and prevents hidden AWS CLI/shared-credential fallback |
| Venue simulation controls | `BacktestVenueConfig` supports routing, frozen account, stop/GTD/contingent controls, bar/trade execution, liquidity consumption, queue position, OTO trigger mode, base currency, default leverage, price boundary, leverage maps, margin model, simulation modules, fill model, latency model, fee model, and settlement prices | manifest maps primitive controls directly into NT: venue name, OMS type, account type, book type, starting balances, routing, frozen account, stop/GTD/contingent toggles, position/random/reduce-only toggles, bar/trade execution toggles, market-order ack toggle, liquidity consumption, cash borrowing, queue position, OTO trigger mode, base currency, default leverage, and price protection points | Uses NT for primitive controls; complex model surfaces pending |
| Converter checkpoint/idempotency | NT skips existing parquet files for identical target intervals, but does not provide Bolt source-proof/converter identity checkpointing | Bolt now owns a thin boundary wrapper that validates source proof, converter identity, checkpoint, manifest, and catalog metadata before rerun/resume | Correct Bolt-owned boundary |
| Source authorization/provenance | NT catalog APIs do not decide whether a raw source is accepted for this project | Bolt owns source proof acceptance and result-contract provenance binding | Correct Bolt-owned boundary |

Conclusion: we are not building a custom backtest engine, fill simulator, catalog format, or NT data loader. We are using NT for those. Bolt is building the governance boundary around NT because NT does not know this project source-proof contract or artifact provenance rules.

## Implemented Fix

Added `crates/backtesting-vertical-slice/src/conversion_boundary.rs`:

- `inspect_conversion_output` rejects non-empty output with no validated checkpoint
- mismatched source proof, object hash, converter identity, converter version, or converter config hash is rejected
- partial output with a valid non-completed checkpoint is resumable
- completed output requires manifest, checkpoint, and catalog metadata
- completed output returns stable conversion manifest, checkpoint, and catalog hashes

Updated `canonical_trades.rs`:

- replaces the venue-specific production converter identity with `csv-native-trades-to-canonical-trades.v1`
- moves raw object-container choice and byte budgets into `[converter.raw_payload]` TOML (`csv_gzip`, `csv_text`, or configured single-member `single_csv_zip`, plus `max_object_bytes` and `max_decoded_bytes`)
- moves raw CSV header mode, column names, timestamp unit, and side token mapping into `[converter.csv]` TOML
- validates converter identity/version through a registered converter list before conversion
- normalizes accepted native-trade CSV rows by resolving configured column names from either the CSV header or, for `has_headers = false`, the accepted source-proof schema columns, so adding another compatible venue/source family does not touch operator, runner, result contract, catalog projection, or NT execution code
- content-hashes the full converter config so container, byte-budget, or mapping changes are provenance-bound

Updated `operator.rs`:

- resolves and validates artifact-store options before running conversion/backtest for publish flows, so misconfigured `s3://` output cannot repeat the slow metadata-service/object-store failure path
- reads converter identity/version, raw payload container, raw payload byte budgets, and CSV mapping from the run spec and validates the registered converter before touching output
- rejects accepted objects larger than `converter.raw_payload.max_object_bytes` before checkpoint writes and bounds decoded CSV reads with `converter.raw_payload.max_decoded_bytes`
- computes the expected conversion fingerprint before output cleanup
- includes converter config hash in the expected conversion fingerprint
- computes the contract-bound run manifest hash before overriding the execution catalog path to the local projection root
- inspects the output directory before deleting any local NT catalog
- writes a started checkpoint before conversion
- writes completed conversion artifacts after the backtest result contract is written
- preserves dirty output evidence when the output directory is invalid
- publishes the verified local artifact tree to `manifest.output_prefix` only through the explicit publish entrypoint
- passes the same resolved artifact-store option map into artifact publish and published-catalog proof

Updated `runner.rs` and `result_contract.rs`:

- builds conversion checkpoint, manifest, and catalog metadata from the accepted dataset and projected NT catalog
- adds `converter_identity`, `converter_version`, `converter_config_hash`, `conversion_manifest_hash`, `conversion_checkpoint_hash`, `catalog_metadata_hash`, and `catalog_metadata_uri` to the result contract
- accepts an explicit `contract_manifest_hash` so operator runs can bind the portable submitted manifest while still letting NT consume the resolved local catalog path
- writes catalog metadata with `output_catalog_uri`, `execution_catalog_uri`, and `direct_s3_catalog_access_proven`; the current operator-localized path records the local execution URI and `false`
- validates those fields as required

Updated `run_manifest.rs`:

- declares primitive NT venue controls in the TOML-backed `ManifestVenueConfig`
- parses and validates NT OTO trigger mode, base currency, default leverage, and price-protection fields
- maps supported controls directly into `BacktestVenueConfig::builder()`
- rejects malformed OTO mode, base currency, and non-positive default leverage before the NT run
- declares NT catalog filesystem protocol and storage option maps in `ManifestCatalogInput`
- maps catalog cloud fields into `BacktestDataConfig::builder()`
- rejects unsupported catalog filesystem protocols before the NT run
- rejects mixed generic/Rust catalog storage option maps because NT `BacktestNode` chooses the Rust-specific map when it is non-empty, which would otherwise silently shadow generic options such as `region`
- rejects S3 catalog storage option keys not consumed by this pinned NT revision, such as `aws_virtual_hosted_style_request`
- declares `[manifest.artifact_store]` storage option maps and optional `[manifest.artifact_store.ssm_parameters]`
- rejects raw S3 credential keys in artifact-store TOML, including `access_key_id`, `secret_access_key`, and `session_token`
- validates SSM parameter paths as absolute paths without whitespace
- resolves artifact-store credentials through an injected resolver and requires resolved S3 access key and secret key before `s3://` publish/proof can proceed

Added `artifact_store_secrets.rs`:

- defines a small `ArtifactStoreSecretResolver` trait for runtime secret lookup
- implements `ArtifactStoreSsmResolver` with `aws_config` and `aws_sdk_ssm`
- caches SSM clients per region and redacts parameter paths from SSM error messages
- does not shell out to AWS CLI, read shared AWS credential files, or introduce environment-variable credential fallback

Updated committed reference artifact:

- `specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-result-contract.bnbusdc-2026-03-01.json` now includes generic converter identity/version, converter config hash, conversion artifact hashes, and catalog metadata binding
- `specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-catalog-metadata.bnbusdc-2026-03-01.json` records the portable reference metadata and explicitly does not claim direct S3 catalog execution
- `specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml` now declares primitive NT venue controls, catalog filesystem fields, `[converter.raw_payload]` container and byte budgets, and `[converter.csv]` mapping explicitly

Updated CLI:

- default run remains local-only
- `--publish-output` runs the local proof first, then copies every produced artifact under `output_dir` to the configured `manifest.output_prefix`
- if `[manifest.artifact_store.ssm_parameters]` is present, the CLI resolves those paths through the Rust AWS SDK and uses the resulting object-store options for publish/proof
- rejects `accepted_object.bytes` above `converter.raw_payload.max_object_bytes` before invoking the local object reader
- local `file://` publish roots are created before opening NT's local object store; remote prefixes are not pre-created locally

## TDD Evidence

RED checks observed before implementation:

- `just bte-test --test backtesting_vertical_slice_conversion_boundary` failed on unresolved `backtesting_vertical_slice::conversion_boundary`
- `just bte-test operator::tests::run_from_run_spec_writes_conversion_artifacts_and_contract_binds_them` failed because `BacktestResultContract` lacked the conversion provenance fields
- `just bte-test venue_config_maps_explicit_nt_venue_controls` failed with `E0609` missing `ManifestVenueConfig` fields such as `routing`, `frozen_account`, and `reject_stop_orders`
- `just bte-test data_config_maps_catalog_cloud_options` failed with `E0609` missing `ManifestCatalogInput` fields for catalog filesystem protocol and storage options
- `just bte-test run_from_run_spec_contract_manifest_hash_is_portable_run_spec_hash` failed because the contract bound the mutated local execution manifest hash `6004a1fc1860ea65a0fdb887efe85a9efef428fe04d157fc5865b06072c8efa7` instead of the portable submitted run-spec manifest hash `921685dd70be98e8a5744b0eda33f9d91303999ab9098d89963a1e3747cd0dd5`
- `just bte-test rejects_shadowed_catalog_storage_options_before_nt_config rejects_unknown_s3_catalog_rust_storage_option_before_nt_config` failed because `to_nt_data_config()` accepted mixed storage option maps and an S3 option key that NT ignores
- `just bte-test clean_new_output_writes_manifest_checkpoint_and_catalog_metadata run_from_run_spec_writes_conversion_artifacts_and_contract_binds_them` failed with missing `ConversionCatalogMetadata.execution_catalog_uri` and `direct_s3_catalog_access_proven` fields
- `just bte-test run_from_run_spec_writes_conversion_artifacts_and_contract_binds_them` failed with missing `catalog_metadata_hash`, `catalog_metadata_uri`, and `ConversionCatalogMetadata::content_hash`
- `just bte-test run_from_run_spec_rejects_unregistered_converter_version` failed because converter version `2` still executed the v1 adapter
- `just bte-test run_from_run_spec_uses_configured_csv_trade_mapping` failed with missing `ConverterConfig.csv`, proving raw CSV column mapping was not yet config-owned
- `just bte-test run_from_run_spec_writes_conversion_artifacts_and_contract_binds_them` failed with missing `converter_config_hash` on converter config, conversion fingerprint, and result contract
- `just bte-test run_from_run_spec_uses_configured_single_csv_zip_payload` failed with missing `RawPayloadConfig`, missing `RawPayloadContainer`, missing `ConverterConfig.raw_payload`, and no ZIP crate, proving object-container selection was still a hidden gzip assumption
- `just bte-test cli_rejects_object_above_configured_payload_max_before_reading_object run_from_run_spec_rejects_object_above_configured_payload_max_before_artifacts run_from_run_spec_rejects_decoded_payload_above_configured_max_before_catalog_work` failed with missing `RawPayloadConfig.max_object_bytes` and `RawPayloadConfig.max_decoded_bytes`, proving raw payload size/expansion limits were not yet TOML-owned
- `just bte-test cli_publish_output_flag_is_explicit_opt_in cli_published_catalog_proof_requires_publish_output cli_publish_preflight_rejects_missing_s3_ssm_before_reading_object` failed after switching the tests to `--object` because `Cli` still exposed `object_gz`
- `just bte-test committed_result_contract_converter_config_hash_matches_run_spec` failed because the committed result contract still carried the pre-`raw_payload` converter config hash `4e54ce1edbdab877a776cb5d38ede603a747da49c0355f80b2f3665905333080` while the run-spec now hashes to `b947a94aa26e2f2391aef02fb9d85073045c08ba6035dcc987e8a73353e4df10`
- `just bte-test committed_result_contract_converter_config_hash_matches_run_spec committed_result_contract_manifest_hash_matches_run_spec committed_result_contract_binds_catalog_metadata committed_result_contract_deserializes committed_run_spec_deserializes` failed after adding byte budgets because the committed result contract still carried converter config hash `b947a94aa26e2f2391aef02fb9d85073045c08ba6035dcc987e8a73353e4df10` while the run-spec now hashes to `a20a83ef7bf42926e394f16c43c09819b58fc8c08d42ceb23352f9a61e293144`
- `just bte-test acceptance_blocked_when_evidence_state_is_not_backfillable` failed because `evaluate_acceptance()` still admitted accepted source proofs whose `evidence_state` was bounded/current-only, pending, vendor/forward-capture-only, not-applicable, or excluded
- `just bte-test acceptance_blocked_when_source_binding_family_disagrees_with_registry` failed because `evaluate_acceptance()` still admitted a proof whose `product_family` disagreed with the registered TOML source binding
- `just bte-test acceptance_blocked_when_source_binding_missing_from_registry` failed because `evaluate_acceptance()` returned `Ok(())` for an unknown `source_binding`, allowing `accept()` to stamp a registry-unknown proof before later object selection rejected it
- `just bte-test rejects_non_latest_proof_pin_without_reason_code` failed because `BacktestingRunManifest::validate()` accepted an audit run that set `pins_non_latest_proof = true` without a structured reason code
- `just bte-test rejects_audit_non_latest_proof_pin_without_reason_detail` failed because `BacktestingRunManifest::validate()` accepted an `audit_or_investigation` proof pin without detail
- `just bte-test accepts_all_configured_non_latest_proof_pin_reason_codes_from_toml` failed because TOML accepted only `baseline_reproduction`, `audit_or_investigation`, and `migration_validation`, while the plan/reference contract also allows `published_result_reproduction` and `regression_comparison`

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
- `just bte-test run_from_run_spec_contract_manifest_hash_is_portable_run_spec_hash`: 1 passed
- `just bte-test data_config_maps_catalog_cloud_options rejects_shadowed_catalog_storage_options_before_nt_config rejects_unknown_s3_catalog_rust_storage_option_before_nt_config`: 3 passed
- `just bte-test clean_new_output_writes_manifest_checkpoint_and_catalog_metadata run_from_run_spec_writes_conversion_artifacts_and_contract_binds_them`: 2 passed
- `just bte-test run_from_run_spec_uses_configured_csv_trade_mapping run_from_run_spec_rejects_unregistered_converter_version`: 2 passed
- `just bte-test run_from_run_spec_writes_conversion_artifacts_and_contract_binds_them`: 1 passed after binding converter config hash and catalog metadata hash
- `just bte-test run_from_run_spec_uses_configured_csv_trade_mapping run_from_run_spec_rejects_unregistered_converter_version run_from_run_spec_writes_conversion_artifacts_and_contract_binds_them committed_result_contract_deserializes committed_result_contract_binds_catalog_metadata accepted_data_flows_through_to_objective_result_contract`: 6 passed
- `just bte-test run_from_run_spec_uses_configured_single_csv_zip_payload`: 1 passed after adding `[converter.raw_payload]`, exact-pinned `zip = 0.6.6`, and a single-member ZIP CSV decoder with a Binance source-binding fixture swap
- `just bte-test cli_rejects_object_above_configured_payload_max_before_reading_object run_from_run_spec_rejects_object_above_configured_payload_max_before_artifacts run_from_run_spec_rejects_decoded_payload_above_configured_max_before_catalog_work`: 3 passed after adding TOML-owned `max_object_bytes`, bounded decoded CSV reads, a CLI pre-read object-max guard, and an operator pre-check before checkpoint writes
- `just bte-test cli_publish_output_flag_is_explicit_opt_in cli_published_catalog_proof_requires_publish_output cli_publish_preflight_rejects_missing_s3_ssm_before_reading_object read_object_rejects_size_mismatch_before_loading_object`: 4 passed after replacing the public CLI input with generic `--object`
- `just bte-test committed_result_contract_converter_config_hash_matches_run_spec committed_result_contract_manifest_hash_matches_run_spec committed_result_contract_binds_catalog_metadata committed_result_contract_deserializes committed_run_spec_deserializes`: 5 passed after updating the reference converter, conversion, checkpoint, and catalog-metadata hashes
- `just bte-test run_from_run_spec_uses_configured_single_csv_zip_payload run_from_run_spec_produces_artifacts run_from_run_spec_uses_configured_csv_trade_mapping run_from_run_spec_writes_conversion_artifacts_and_contract_binds_them committed_result_contract_converter_config_hash_matches_run_spec committed_result_contract_manifest_hash_matches_run_spec committed_result_contract_binds_catalog_metadata committed_result_contract_deserializes committed_run_spec_deserializes accepted_data_flows_through_to_objective_result_contract`: 10 passed
- `just bte-test artifact_store_resolves_s3_credentials_from_ssm_parameters artifact_store_rejects_raw_s3_credentials_in_toml published_catalog_manifest_uses_resolved_artifact_store_options production_rust_does_not_hardcode_sample_venue_or_instrument cli_published_catalog_proof_requires_publish_output`: 5 passed
- `just bte-test run_from_run_spec_and_publish_rejects_s3_without_ssm_before_running_backtest artifact_store_rejects_s3_publish_without_resolved_ssm_credentials`: 2 passed
- `just bte-test rejects_unsupported_nt_venue_model_surface_requests_before_nt_config rejects_unsupported_nt_engine_surface_requests_before_nt_config`: 2 passed
- `just bte-test alternate_venue_provider_swap_is_toml_only`: 1 passed
- `just bte-test artifact_root_resolves_typed_subpaths_without_extra_root_knobs rejects_unsupported_artifact_root_scheme`: 2 passed after RED compile failure on missing typed artifact-subpath API
- `just bte-test run_from_run_spec_rejects_object_byte_count_mismatch_before_artifacts`: RED failed because the operator ran NT instead of rejecting the byte mismatch, then GREEN passed after adding the pre-hash/pre-decompression byte-count check
- `just bte-test read_object_rejects_size_mismatch_before_loading_object`: RED failed with missing checked-read helper, then GREEN passed after adding CLI object-size preflight before `fs::read`
- `just bte-test backtest_index_record_generates_paths_under_single_artifact_root artifact_index_rejects_missing_lineage_and_non_sha256_hashes artifact_index_rejects_consumer_mutation_of_producer_records`: RED failed with missing `artifact_index` module, then GREEN passed after adding the pure Artifact Index contract helper
- `just bte-test lifecycle_config_rejects_default_delete_or_expiration_rules lifecycle_state_follows_configured_quiet_window lifecycle_config_requires_all_storage_profiles`: RED failed with missing lifecycle config types, then GREEN passed after adding the pure lifecycle policy helper
- `just bte-test committed_snapshot_resolution_rejects_hash_invalid_latest_pointer committed_snapshot_resolution_rejects_stale_latest_pointer committed_snapshot_resolution_requires_hot_index_metadata_active committed_snapshot_rejects_staged_or_orphan_records_as_discovery_truth latest_pointer_update_plan_uses_create_or_etag_preconditions`: RED failed with missing committed-snapshot/latest-pointer contract types, then GREEN passed after adding the pure Artifact Index commit-path helpers
- `just bte-test cross_kind_parent_resolution_uses_manifest_lineage_hashes cross_kind_parent_resolution_rejects_independent_latest_parent_hash`: RED failed with missing lineage-parent resolver, then GREEN passed after adding manifest-lineage parent resolution by artifact id and `sha256` hash
- `just bte-test immutable_event_create_is_idempotent_for_same_payload immutable_event_create_rejects_different_payload_at_same_uri`: RED failed with missing event object/create-plan contract types, then GREEN passed after adding the pure immutable event-create helper
- `just bte-test data_config_preserves_configured_object_store_conditional_put artifact_store_preserves_conditional_put_after_ssm_resolution artifact_store_rejects_disabled_conditional_put_for_s3_commit_path`: RED failed because the manifest rejected `conditional_put`; GREEN passed after preserving canonical `conditional_put = "etag"` through NT/object-store options and rejecting `disabled` for S3 Artifact Index commit readiness
- `just bte-test acceptance_blocked_when_evidence_state_is_not_backfillable`: GREEN passed after source-proof acceptance began rejecting non-backfillable evidence states
- `just bte-test acceptance_blocked_when_source_binding_family_disagrees_with_registry`: GREEN passed after source-proof acceptance began binding proof `product_family`, `table_family`, and `evidence_state` to the TOML source-binding registry
- `just bte-test acceptance_blocked_when_source_binding_family_disagrees_with_registry acceptance_blocked_when_source_binding_missing_from_registry select_rejects_unknown_source_binding select_rejects_object_from_other_venue select_accepts_configured_source_host_with_url_variations`: 5 passed after source-proof acceptance began rejecting registry-missing source bindings and selection kept a forged-record guard
- `just bte-test select_rejects_same_host_path_outside_declared_source_template`: RED failed because a same-host Binance monthly trades object could satisfy the daily native-trades source binding; GREEN passed after selection began matching the object `source_url` path/query against the registered URI template
- `just bte-test rejects_non_latest_proof_pin_for_normal_run rejects_non_latest_proof_pin_without_reason_code rejects_audit_non_latest_proof_pin_without_reason_detail accepts_non_latest_reproduction_pin_with_reason_code`: 4 passed after adding typed non-latest proof-pin reason fields
- `just bte-test accepts_all_configured_non_latest_proof_pin_reason_codes_from_toml`: 1 passed after adding the missing `published_result_reproduction` and `regression_comparison` enum variants
- `just bte-test publish_output_artifacts_rejects_existing_published_artifact_without_overwrite`: RED failed because publish used default object-store overwrite semantics, then GREEN passed after bounded target preflight plus `PutMode::Create`
- `just bte-test publish_output_artifacts_rejects_existing_published_artifact_without_overwrite run_from_run_spec_and_publish_can_prove_published_catalog_consumption`: 2 passed after proof-mode publish stopped publishing the metadata/contract artifacts before the proof updates them
- `just bte-test cli_publish_preflight_rejects_missing_s3_ssm_before_reading_object`: RED failed because the CLI had no injectable pre-object-read publish preflight seam, then GREEN passed after publish storage options were resolved before local object read
- `just bte-test typed_unsupported_nt_venue_model_surfaces_parse_then_fail_before_nt_config`: RED failed with `E0599` because `ManifestError::UnsupportedNtSurface` and typed unsupported-surface schema did not exist
- `just bte-test rejects_unsupported_nt_venue_model_surface_requests_before_nt_config typed_unsupported_nt_venue_model_surfaces_parse_then_fail_before_nt_config`: 2 passed after adding optional manifest placeholders for NT leverage maps, margin model, modules, fill model, latency model, fee model, and settlement prices plus structured pre-NT rejection
- `just bte-test typed_unsupported_nt_catalog_query_surfaces_parse_then_fail_before_nt_config`: RED failed because `instrument_ids` under `[catalog_input]` was an unknown TOML field, proving pinned NT `BacktestDataConfig` query surfaces were not represented even as explicitly unsupported surfaces
- `just bte-test native_trade_source_bindings_cover_multiple_configured_venues`: RED failed with only `venues={"bybit"}` and `keys=["bybit-spot-tick-trades"]`; GREEN passed after adding `binance-spot-native-trades` as a registry-only candidate and requiring the test to exercise proof acceptance plus host selection for each configured native-trades binding
- `just bte-test acceptance_blocked_when_structured_scope_summary_missing`: RED failed with `E0609` because `SourceProofReport` had no `acceptance_scope` field; GREEN passed after adding structured scope facts and requiring them before proof acceptance
- `just bte-test acceptance_blocked_when_structured_scope_summary_has_failures_or_scope_violations`: RED failed because `evaluate_acceptance()` returned `Ok(())` with `failed_objects = 1`; GREEN passed after rejecting failed objects and selector-scope violations in the structured scope summary
- `just bte-test ledger_rejects_object_bytes_exceeding_structured_acceptance_scope`: RED failed because `select_accepted_dataset()` admitted an object whose bytes exceeded `acceptance_scope.accepted_bytes`; GREEN passed after object selection compares selected object bytes against the structured accepted byte count
- `just bte-test non_l2_fidelity_requires_structured_claim_limits structured_claim_limits_must_cover_forbidden_claims`: RED failed with `E0609` because `SourceProofReport` had no `claim_limits` field; GREEN passed after adding structured source-proof claim-limit rows and requiring every non-L2 `forbidden_claims` entry to be covered by a machine-readable limit
- `just bte-test accepted_data_flows_through_to_objective_result_contract`: RED failed because generated result contracts dropped structured source-proof claim-limit rows after acceptance and retained only plain canonical-table `forbidden_claims`; GREEN passed after `AcceptedDataset` began carrying `claim_limits` and runner/operator result-contract assembly consumed those structured rows
- `just bte-test --test backtesting_vertical_slice_end_to_end accepted_data_flows_through_to_objective_result_contract`: RED failed because the result contract claim limits carried only source-fidelity limits and no NT surface/default records; GREEN passed after generated result contracts appended resolved NT default, pass-through, and unsupported-surface claim-limit entries derived from `BacktestRunConfig`
- `just bte-test committed_result_contract_records_nt_extension_surface_claim_limits`: RED failed because the checked-in reference result contract lacked NT extension-surface claim limits; GREEN passed after updating the reference fixture
- `just bte-test run_from_run_spec_writes_resolved_run_manifest_artifact`: RED failed with missing `BacktestRunManifestArtifact`, `NtSurfaceClassification`, and `RunArtifacts.run_manifest_path`; GREEN passed after adding the portable `backtest-run-manifest.json` artifact with submitted manifest hash, submitted manifest, and structured resolved NT surface records
- `just bte-test resolved_nt_surfaces_record_supported_manifest_to_nt_mappings`: RED failed because `resolved_nt_surfaces()` omitted supported venue/catalog pass-through records such as `venue.name`; GREEN passed after each successful run began recording supported `BacktestVenueConfig` and `BacktestDataConfig` mappings, with catalog storage-option values redacted to key lists
- `just bte-test run_from_run_spec_writes_portable_redacted_contract`: RED failed because the new `catalog.catalog_path` claim-limit row exposed the local temp NT catalog path; GREEN passed after operator contract redaction began replacing the local catalog root with the portable result-contract catalog URI
- `just bte-test typed_unsupported_nt_catalog_query_surfaces_parse_then_fail_before_nt_config resolved_nt_surfaces_record_unsupported_catalog_query_mappings`: GREEN passed after declaring NT data-query surfaces as typed unsupported TOML fields, rejecting them before NT config construction, and recording them in resolved NT surfaces without adding venue/data-family-specific branches
- `just bte-test run_manifest_unit_tests_do_not_embed_accepted_sample_fixture_values`: RED failed with `bybit, bnbusdc` in generic `run_manifest` unit fixtures; GREEN passed after adding a test-only synthetic accepted dataset constructor and replacing those fixtures with synthetic source proof, binding, venue, instrument, and bar-type values
- `just bte-test --test backtesting_vertical_slice_backfill_coverage`: RED failed with `E0432` because the run-level `backfill_coverage` module did not exist; GREEN passed after adding a venue-agnostic coverage ledger that classifies normalized manifest evidence and physical inventory summaries as accepted, accepted-with-gaps, rejected, or physical-only before any download, canonical write, NT catalog projection, or backtest
- `just bte-test --test backtesting_vertical_slice_backfill_coverage`: RED failed with `E0432`/`E0599` because `BackfillCoverageParseError` and `BackfillCoverageManifestEvidence::from_manifest_json` did not exist; GREEN passed after adding a schema-field-alias parser for manifest summaries, including top-level and nested count aliases, inferred planned-object accounting, source-proof status injection, and explicit unknown-write-mode rejection
- `just bte-test --test backtesting_vertical_slice_backfill_coverage`: RED failed with `E0432` because `BackfillCoverageLedger` and `BackfillCoverageLedgerError` did not exist; GREEN passed after adding a deterministic schema-versioned ledger aggregate from normalized manifest evidence plus physical inventory summaries, including duplicate manifest/inventory guards, source-proof id preservation, summary totals, and a JSON content hash
- `just bte-test --test backtesting_vertical_slice_backfill_coverage`: RED failed with `E0432` because `BACKFILL_COVERAGE_LEDGER_FILE`, `BackfillCoverageWriteError`, and `write_coverage_ledger_artifact` did not exist; GREEN passed after adding a local `backfill-coverage-ledger.json` writer that creates the output directory, records the deterministic ledger hash/byte/record counts, admits same-content reruns, and rejects mismatched existing artifacts without overwrite
- `just bte-test --test backtesting_vertical_slice_backfill_coverage`: RED failed with `UnknownWriteMode("s3_staging_only")` after adding observed real-manifest aliases; GREEN passed after accepting the generic staging-only write-mode alias plus `object_count_excluding_manifest` and `bytes_excluding_manifest` manifest-count aliases
- `just bte-test --test backtesting_vertical_slice_backfill_coverage`: RED failed with missing `BackfillCoverageManifestJson`, missing `BackfillCoverageLedger::from_manifest_json_summaries`, and missing manifest-URI parse error reporting; GREEN passed after adding a batch manifest-summary ingestion boundary that converts many manifest JSON summaries into ledger evidence without reading raw payloads and reports the offending manifest URI on parse failure
- `just bte-test --test backtesting_vertical_slice_backfill_coverage`: RED failed with missing `BackfillCoverageManifestFile`, missing file-level JSON parse errors, and missing manifest-file artifact writer; GREEN passed after adding a local manifest-file reader that writes `backfill-coverage-ledger.json` from manifest JSON files without reading raw payload bytes and preserves manifest URI/path evidence on invalid JSON
- `just bte-test --test backtesting_vertical_slice_backfill_coverage`: RED failed with missing `write_coverage_ledger_artifact_from_spec_file`; GREEN passed after adding a config-owned TOML coverage spec with `ledger_id`, `[[manifest]]`, and optional `[[inventory]]` rows so manifest-file ledger generation is driven by TOML rather than command-line runtime values
- `just bte-test --test backtesting_vertical_slice_backfill_coverage_cli`: RED failed because no `backfill_coverage_ledger` binary target existed for the config-owned coverage spec; GREEN passed after adding a thin operator command that accepts only `--spec`, reads `output_dir` from TOML, writes the idempotent ledger artifact, and prints aggregate coverage counts without reading payload data
- `just bte-test --test backtesting_vertical_slice_backfill_coverage`: RED failed because `[[manifest]]` TOML entries could not carry source-proof binding metadata when real manifest summaries lacked `source_binding`, `source_proof_id`, or `source_proof_version`; GREEN passed after adding generic optional TOML bindings for those fields before manifest parsing, without adding venue/data-family constants
- static provider-literal scan for the coverage source, coverage CLI, and their tests: no hits for current venue/provider/sample tokens, so the new coverage-ledger API, operator command, and tests are not hardcoded to the accepted sample or a specific venue
- `just bte-test research_analytics_artifacts_use_typed_subfamilies_and_one_kind_pointer research_analytics_records_require_matching_subfamily_prefix`: RED failed with missing `ResearchAnalyticsSubfamily` and RA-specific staged-record constructor; GREEN passed after adding typed RA subfamilies, enforcing `research-analytics/v1/<subfamily>/` manifest prefixes, and keeping every RA subfamily on the single `research_analytics` Artifact Index pointer
- `just bte-test approved_for_config_requires_objective_evidence_and_non_live_boundary promotion_package_rejects_proof_strength_upgrade_and_forbidden_actions promotion_package_artifacts_must_live_under_ra_promotion_family promotion_package_rejects_notebook_to_production_direct_promotion approved_for_config_accepts_preserved_claim_limited_typed_config_only`: RED failed with missing `research_analytics` module; GREEN passed after adding a pure `PromotionPackage` validator with canonical status enum, accepted source-proof refs, objective BTE result refs, preserved claim limits, fidelity upgrade rejection, notebook/runtime boundary checks, typed config artifact checks, reviewer-policy refs, and RA-owned promotion-family URI validation
- `just bte-test promotion_package_preserves_dashboard_field_refs_as_read_only_metadata`: RED failed with missing `dashboard_field_refs`; GREEN passed after adding dashboard-facing reference metadata without giving RA authority to mutate upstream BTE/source-proof artifacts
- `just bte-test promotion_package_rejects_proof_strength_upgrade_and_forbidden_actions`: RED failed with missing `accepts_source_proofs`; GREEN passed after making unauthorized source-proof acceptance an explicit forbidden promotion-package behavior
- `just bte-test promotion_package_rejects_cross_family_fidelity_claims`: RED failed because a rank-only check allowed snapshot-replay evidence to support trade-replay claims; GREEN passed after replacing the rank with an explicit source-fidelity-to-claim compatibility matrix
- `just bte-test accepts_human_typed_strategy_config_with_artifact_hash accepts_research_analytics_promotion_package_strategy_config rejects_research_analytics_strategy_config_without_package_ref rejects_research_analytics_strategy_config_outside_promotion_family rejects_notebook_runtime_strategy_source`: RED failed with missing `StrategySourceKind`, strategy config provenance fields, and strategy-source error variants; GREEN passed after adding explicit `compiled_rust_registry`, `human_typed_config`, and `research_analytics_promotion_package` source kinds. Execution still resolves only registered compiled Rust strategies; human/RA sources add immutable typed-config/package URI+hash provenance and RA promotion artifacts must live under `artifact_root/research-analytics/v1/promotion-packages/`.
- `just bte-test run_from_run_spec_reuses_completed_output_without_rebuilding_catalog`: RED failed because a second run into a completed output deleted the existing NT catalog root; GREEN passed after `ConversionOutputState::Complete` began loading the proven canonical Parquet artifact, recomputing the logical NT catalog hash, running NT read-back/BacktestNode checks, and preserving the completed conversion checkpoint/manifest/catalog metadata plus catalog root
- `just bte-test run_from_run_spec_reuses_completed_output_without_rebuilding_catalog run_from_run_spec_accepts_completed_output_on_second_run`: 2 passed after the completed-output reuse path was added
- `just bte-test`: 238 passed, including 2 slow public API tests
- `just bte-test`: 247 passed after structured source-proof claim-limit propagation into generated result contracts
- `just bte-test`: 249 passed after typed unsupported NT catalog-query gating and test-literal cleanup
- `just bte-test`: 250 passed after moving generic `run_manifest` unit tests off accepted-sample proof literals
- `just bte-test`: 255 passed after adding the run-level backfill coverage ledger, including 5 new provider-agnostic coverage tests and 2 slow public API compile-fail tests
- `just bte-test`: 258 passed after adding the generic manifest-summary parser to the backfill coverage ledger, including 8 provider-agnostic backfill coverage tests and 2 slow public API compile-fail tests
- `just bte-test`: 261 passed after adding the deterministic backfill coverage ledger aggregate, including 11 provider-agnostic backfill coverage tests and 2 slow public API compile-fail tests
- `just bte-test`: 263 passed after adding the idempotent local backfill coverage ledger artifact writer, including 13 provider-agnostic backfill coverage tests and 2 slow public API compile-fail tests
- `just bte-test`: 264 passed after adding observed real-manifest alias support, including 14 provider-agnostic backfill coverage tests and 2 slow public API compile-fail tests
- `just bte-test`: 266 passed after adding batch manifest-summary ingestion, including 16 provider-agnostic backfill coverage tests and 2 slow public API compile-fail tests
- `just bte-test`: 268 passed after adding local manifest-file coverage-ledger artifact writing, including 18 provider-agnostic backfill coverage tests and 2 slow public API compile-fail tests
- `just bte-test`: 269 passed after adding TOML coverage-spec driven ledger generation, including 19 provider-agnostic backfill coverage tests and 2 slow public API compile-fail tests
- `just bte-test`: 270 passed after adding the config-owned backfill coverage CLI, including 19 provider-agnostic backfill coverage tests, 1 provider-agnostic coverage CLI integration test, and 2 slow public API compile-fail tests
- `just bte-test`: 271 passed after adding generic TOML source-proof metadata binding, including 20 provider-agnostic backfill coverage tests, 1 provider-agnostic coverage CLI integration test, and 2 slow public API compile-fail tests
- `just bte-fmt-check`: passed
- `just bte-clippy`: passed
- `just bte-build`: passed
- `just source-fence`: passed
- `git diff --check`: passed
- static `bybit|BYBIT|BNBUSDC|bnbusdc|public_archive|source-proof-bybit|bybit-spot` scan for `src/run_manifest.rs`: no hits after the synthetic unit-fixture cleanup; broader `src/` sample hits remain confined to source-proof/catalog/operator tests or committed reference-artifact checks, not production operator/runner/converter control flow
- rebuilt binary local accepted-object run: exit 0, `937` canonical rows, `937` NT read-back ticks, `937` NT iterations, stable catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`
- current-branch local accepted-object run after SSM changes: exit 0, `937` canonical rows, `937` NT read-back ticks, `937` NT iterations, stable catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`
- current-branch misconfigured S3 publish/proof run: exit 1 before NT backtest logs, explicit missing SSM binding error, no output directory created
- rebuilt binary local `--publish-output` run to a `file://` prefix: exit 0, 8 published artifacts, published tree matched local output tree byte-for-byte

## Remaining Unknowns And Concrete Solutions

| Unknown | Why it matters | Solution path |
| --- | --- | --- |
| Clean production output proof under `nt-research-analytics/` | The prefix is empty, so there is no S3 proof of a clean catalog/result path | configure valid artifact-store SSM parameter paths, run the accepted BNBUSDC object through the operator into the configured prefix, upload checkpoint/manifest/metadata/catalog/contract, then verify S3 listing and hashes |
| Artifact-store SSM parameter paths | Direct S3 proof cannot use AWS CLI/shared-credential fallback under repo rules, and current non-secret SSM name searches found no obvious artifact/S3 credential parameters | create or provide SSM parameter paths for S3 access key id and secret access key, optionally session token, then add only those paths to `[manifest.artifact_store.ssm_parameters]`; never put secret values in TOML or logs |
| Broader source proof coverage | This slice proves one Bybit spot trade-replay object only, as a sample source; registry coverage now has Bybit and Binance native-trades candidates, and the converter can process headerless Binance-style ZIP CSV, but Binance is not accepted input. The existing Binance staging manifest has monthly BNBUSDC trades, not the configured daily BNBUSDC object, and same-host monthly paths are now rejected under the daily binding | accept additional source proofs only after raw/schema samples are staged under S3 artifact storage, checksums/hashes are bound, license/retention evidence is recorded, and historical instrument-universe proof exists; for compatible CSV native-trade sources, add proof/run-spec `[converter.csv]` mapping without changing operator/runner/NT code; if accepting monthly Binance files, add a monthly source binding/proof path rather than reusing the daily binding; for non-CSV or non-trade data, add a new registered adapter and bind its converter config hash |
| Broader NT data-query surface | The manifest now declares pinned NT `BacktestDataConfig` query surfaces as typed unsupported fields, rejects them before NT config construction, and records them in resolved-surface/result-contract claim limits; real multi-instrument/query/filter/bar mappings are still not implemented | keep the slice single-instrument trade replay until each additional NT query knob has a typed source-proof/fidelity implication and a real mapping into NT; do not add ad hoc query strings, venue branches, or hidden defaults |
| Complex NT venue model policy | Primitive controls are explicit, and complex surfaces are manifest-declared with structured unsupported-surface rejection, but leverage maps, margin model, simulation modules, fill model, latency model, fee model, and settlement prices are not yet mapped into NT | keep them `unsupported_for_now` until each field has a claim-limited typed mapping into NT `BacktestVenueConfig`; do not silently rely on hidden defaults |
| Direct S3 catalog execution proof | The proof path is implemented to pass resolved object-store options into NT, but it has not run against real S3 because SSM credential parameter paths are missing | after SSM paths are configured, run `--publish-output --prove-published-catalog`, verify S3 artifact hashes, and require the published-catalog proof to stamp `direct_s3_catalog_access_proven = true` |
| Artifact Index commit proof | The pure record and committed-discovery contracts are now validated locally, including pointer/snapshot hash checks, stale pointer rejection, staged/orphan rejection, active hot metadata, manifest-lineage parent resolution, immutable event idempotency/overwrite rejection, and first-write/update precondition modeling. The pinned object_store API and AWS backend support the needed create-only/update-if-etag primitives when `conditional_put = "etag"` is configured. Real S3 create-only object writes, actual conditional latest-pointer swaps, persisted snapshot serialization, conditional-failure retry/rebase against storage state, audit epoch object creation, and producer IAM scopes are not proven | keep events/snapshots proof-gated; after valid artifact-store SSM paths exist, run a small S3 Artifact Index commit/readback proof using object_store `PutMode::Create` for event/snapshot objects and `PutMode::Update(UpdateVersion)` for latest pointers; if real S3 rejects those semantics, select an approved coordinator/table format before relying on Artifact Index for committed discovery |
| Artifact lifecycle operations | Lifecycle policy is validated in-process, but S3 bucket/object lifecycle rules, transition execution, restore behavior, and active storage for hot pointers/current snapshots are not proven | keep runtime deletes/expirations disabled by contract; model lifecycle costs and prove storage-class transition/restore behavior before enabling any artifact-store policy |
| Old partial output disposition | Old outputs must not be promoted as clean | keep old outputs marked partial/dirty; after clean replacement exists, retain or archive them as forensic evidence, but do not use them as accepted result artifacts |

## Prior Backfill Slow-Run Root Cause

This section is evidence for what must not be repeated. It is based on committed
handoff documents, current S3 manifest reads, and historical converter commits;
no broad backfill or NT-catalog conversion was rerun during this investigation.

Confirmed root causes:

- Scope explosion: the one-off backfill staged hundreds of GB before a
  machine-readable acceptance ledger existed. The 2026-06-02 handoff records
  418,562,436,536 accepted manifest-backed staged bytes including Polymarket
  PMXT source, while also saying this is not canonical normalized data and not
  NT backtesting input.
- Local-retention pressure: the status report records old raw/progress caches
  under `/private/tmp` and a later retention patch that deletes transient Bybit
  payload files after S3 upload. BTE must not retain raw payload mirrors after
  upload/hash verification.
- Duplicate Bybit REST work: the status report records that previous Bybit
  archive chunks also ran the full three-month REST tranche independently. The
  Bybit REST-like manifest `run=bybit-backfill-run-d7698a37f210ee6b` still
  records only 85 payload objects, 334,206,468 payload bytes, and remaining work
  including full one-year all-symbol archive tick trade staging and full
  one-year all-symbol REST pagination.
- Provider-rate/source errors: the Deribit accepted-partial manifest
  `run=deribit-3m-35afe0a04aa50c41` records 7,544 raw objects, 15,346,229 raw
  bytes, and 1,118 errors, mostly HTTP 429. The first errors include invalid
  DOGE/HYPE instrument-universe combinations returning HTTP 400. Retrying this
  shape blindly would repeat slow failed work.
- Acceptance mismatch after upload: the OKX target manifest
  `run=okx-3m-8e300a494d2bd6e1` records zero script errors but is rejected from
  accepted totals because payload selectors include `ALL_SWAP`, which violates
  the base-ticker filter. A run can be technically complete and still unusable.
- Completed-output operator gap: the conversion boundary already classified a
  matching completed checkpoint/manifest/catalog-metadata chain as `Complete`,
  but `run_from_run_spec` treated `Complete` the same as clean/resumable output.
  It wrote a started checkpoint, deleted the NT catalog root, decompressed the
  raw object, and reran conversion/catalog projection. The fixed path now
  reuses completed output only after re-verifying the accepted object SHA,
  loading the canonical Parquet artifact, recomputing the logical NT catalog
  hash from NT reads, proving read-back count, and running BacktestNode against
  the verified catalog.
- Partial manifest coverage: the handoff records PMXT physical S3 objects that
  exceed accepted manifest-backed objects. A fresh S3 check on 2026-06-06
  returned 915 objects and 344,758,798,407 bytes under
  `s3://bolt-parquet/backfill-staging/2026-06-01/polymarket-pmxt-v2-streaming/`,
  while the old handoff counted 914 objects and 344,758,628,885 bytes. Current
  object presence is not acceptance.
- Converter fail-fast/OOM blockers: historical NT-catalog conversion commits
  record that the old conversion loop aborted whole runs on one bad object, and
  that the Binance aggTrades path decompressed a large ZIP member into a huge
  string plus cloned per-row provenance, causing an EXIT=137 OOM on a 32 GB
  host. Later commits added per-object failure isolation, streaming Binance
  aggTrades conversion, and bounded staged object reads. Those fixes are
  necessary evidence, not permission to rerun broad conversion without scope
  gates.
- Source-proof schema mismatch: `backfill-source-proof.v1` says accepted
  canonical backfill claims require `directly_backfillable` or
  `owner_archive_backfillable`, but the Rust acceptance gate previously did not
  enforce that field. That meant a bounded/current-only, pending-source-proof,
  vendor/forward-capture-only, not-applicable, or excluded record could pass
  `evaluate_acceptance()` if its required checks were otherwise marked passed.
- Source-binding metadata mismatch: the TOML registry carries
  `product_family`, `table_families`, and `evidence_state`, but the Rust
  acceptance path previously used the registry only for source URL host
  validation. A proof could reuse a registered source binding while changing
  the data family or acceptance state, making venue/data-type additions less
  systematic than the registry implied.
- Source-binding existence mismatch: after metadata binding was added, the
  acceptance path still treated a missing registry row as "nothing to compare"
  and returned success. Unknown bindings were rejected later only during object
  selection, so an invalid accepted proof record could still be stamped.
- Non-latest proof-pin policy mismatch: the spec requires normal runs to use
  latest accepted proofs and non-normal proof pins to carry a structured
  `proof_pin_reason_code`, with detail for `audit_or_investigation`, but the
  manifest previously had only a boolean `pins_non_latest_proof`. Non-normal
  runs could therefore pin old proof versions without recording why.
- Proof-pin vocabulary mismatch: the plan/reference contract allows
  `published_result_reproduction` and `regression_comparison`, but the Rust
  enum previously omitted them. TOML manifests using those documented reason
  codes failed deserialization before validation, so the governance vocabulary
  was not actually usable end to end.
- Published-output overwrite mismatch: object-store `put` defaults to
  overwrite/upsert, so a dirty or partially populated publish prefix could be
  overwritten by a new run. That violated the clean-output proof requirement
  and could hide stale converted artifacts. Publish now performs a bounded
  target preflight and writes with object-store `PutMode::Create`; proof-mode
  publish withholds the metadata and result-contract files until the
  published-catalog proof updates them, avoiding intentional double-writes.
- CLI publish preflight ordering mismatch: the operator rejected missing S3
  artifact-store credentials before conversion/backtest, but the CLI read the
  accepted object before reaching that operator preflight. For large accepted
  objects, a missing SSM setup could still waste local object I/O. The CLI now
  resolves publish storage options before reading the object and reuses the
  resolved map for the publish/proof run.

No-repeat controls before any future broad backfill or BTE conversion:

1. Start from an accepted source proof and a bounded object list, not a venue
   sweep.
2. Estimate planned object count and byte count from manifests/source indexes
   before downloading payloads.
3. Enforce the accepted-object byte count before hashing, decompression,
   checkpoint writes, or NT work; CLI callers must stat the local object before
   reading it into memory.
4. Require a manifest schema with planned, completed, failed, skipped, bytes,
   hashes, gap reasons, and source-proof ids before a run can be accepted.
5. Require each staged object's source URL to match the accepted source
   binding's configured URI template; matching the HTTPS host alone is not
   sufficient.
6. Fail preflight on unbounded selectors such as `ALL_SWAP`, `baseCoin=all`, or
   "full all-symbol" unless the operator explicitly approves that scope.
7. Use streaming or bounded readers for compressed/archive payloads; never
   decode whole large archives into memory. The current vertical slice now
   enforces TOML-owned `max_object_bytes` before object read/hash work and
   `max_decoded_bytes` during gzip/plain/ZIP CSV decoding.
8. Isolate failures per object or per declared binding so one bad source object
   cannot abandon later good objects, while still exiting non-zero when any
   object failed.
9. Delete transient local payload mirrors after S3 upload and hash verification;
   retain only manifests, source proofs, checkpoints, and small metadata.
10. Treat raw S3 objects without completed accepted manifests as physical
   evidence only, not backtest input.
11. Convert to NT catalog only after source proof, normalized row contract,
    instrument metadata, and gap policy are accepted.
12. For this BTE branch, do not run a broad historical backfill. The only
    admissible next runtime proof is the single accepted BNBUSDC object unless
    a separate source proof and bounded plan are accepted first.
13. Adding a compatible native-trade CSV source must mean adding source proof
    plus `[converter.raw_payload]` and `[converter.csv]` mapping, including
    `has_headers`, not adding venue branches to operator, runner, result
    contract, catalog projection, or NT execution code.
14. Accepted source proofs must carry a backfillable evidence state; bounded or
    pending evidence may be recorded for research, but not promoted into
    canonical BTE input.
15. Accepted source proofs must match the configured source-binding registry's
    product family, table family, and evidence state; adding a compatible source
    means updating registry/source-proof/run-spec TOML, not changing operator or
    runner branches.
16. Accepted source proofs must reference a configured source-binding/venue row;
    unknown binding keys fail at proof acceptance, not only at later object
    selection.
16. Accepted source proofs must name staged `s3://` raw/schema sample artifacts,
    and selected manifest objects must exactly match the proof's
    `raw_sample_uri`; public exchange URLs are source evidence, not accepted raw
    sample storage.
17. Non-latest source-proof pins must be explicit and auditable: `normal` runs
    cannot pin them, and every allowed non-normal pin must carry the structured
    reason fields required by the manifest.
18. The Rust manifest enum, reference contract, and plan vocabulary for
    `proof_pin_reason_code` must stay in lockstep; every documented reason code
    needs a TOML deserialization test.
19. Published artifacts must be create-only under the configured output prefix;
    an existing target object is dirty-prefix evidence and must reject the run
    instead of being overwritten.
20. Publish-path artifact-store validation must happen before reading the local
    accepted object, not merely before conversion/backtest.
21. A completed conversion output must be a verified reuse path, not permission
    to delete and rebuild the NT catalog. Reuse requires matching source proof,
    accepted object hash, converter identity/version/config hash, completed
    checkpoint/manifest/catalog metadata, canonical Parquet source-proof
    binding, logical NT catalog hash, NT read-back count, and BacktestNode
    iteration checks.

## Backfill-First Status

As of the 2026-06-07 bounded S3 recheck, the overall BTE is blocked on accepted
data, not on custom simulator construction.

Current evidence:

- `s3://bolt-parquet/nt-research-analytics/` is still empty:
  `list-objects-v2` returned `KeyCount: 0`. There is no production NT catalog,
  result contract, or published BTE artifact under the clean output prefix.
- `s3://bolt-parquet/backfill-staging/2026-06-01/` has staging prefixes for
  Binance, Bybit, Chainlink, Deribit, Hyperliquid core/targeted/HIP-3/HIP-4,
  OKX, Polymarket PMXT streaming/page1, and source-proof-v3. These are staging
  inputs/evidence, not BTE outputs.
- Representative manifest inspection confirms the current data state is raw
  staging: Binance, OKX, PMXT, and Bybit representative manifests declare
  staging-only write modes and `canonical_s3_write = false`.
- Bounded read-only schema inspection found multiple manifest-summary shapes
  that the ledger must normalize before any broad download: top-level completed
  payload counts, nested `counts.payload_*` fields, top-level
  `completed_object_count`/`completed_bytes`, and
  `object_count_excluding_manifest`/`bytes_excluding_manifest`. The current
  parser accepts these as generic aliases rather than provider-specific code.
- Manifest-only S3 metadata copy for
  `s3://bolt-parquet/backfill-staging/2026-06-01/` copied 190 local
  `*manifest.json` files into `/private/tmp/bte-coverage-ledger-20260607/`;
  no raw payload files were copied. Family counts were: Binance 3, Bybit 15,
  Chainlink 1, Deribit 11, Hyperliquid core 10, Hyperliquid targeted core 1,
  Hyperliquid HIP-3 6, Hyperliquid HIP-4 5, OKX 116, PMXT page1 2, PMXT
  streaming 18, and source-proof-v3 2.
- Current coverage parser supports 166 of those 190 manifest files. The 24
  unsupported manifest schemas are metadata gaps, not payload gaps: Chainlink
  and Deribit manifests have counts/bytes but no `write_mode`; Hyperliquid
  HIP-3/HIP-4 and one source-proof-v3 manifest have `write_mode` but no
  normalized completed-object/byte totals.
- The supported-shape coverage ledger was generated at
  `/private/tmp/bte-coverage-ledger-20260607/ledger-output-supported/backfill-coverage-ledger.json`
  with content hash
  `ebe6363148bf53e2012bb4d013b2a8086abfdb7addcc738641a96561bd867a41`.
  It contains 166 records, all rejected: 166 `missing_source_proof`, 145
  `empty_source_binding`, 5 zero-planned/completed object/byte records, and 3
  failed-object records.
- Canonical source-proof reports under
  `source-proof-v3/source-proofs/v1/**/source-proof.json` total 21, and all 21
  are currently `pending`. After TOML source-proof binding, the bound coverage
  ledger at
  `/private/tmp/bte-coverage-ledger-20260607/ledger-output-bound/backfill-coverage-ledger.json`
  has content hash
  `5f4f5e9bd991ce5508c5445d8db3e6056b7a3e91efa99fd1f19bd96c3c11708c` and
  still has 0 accepted records: 20 records are now specifically blocked by
  `source_proof_not_accepted`, while 146 records still have no source-proof
  binding.
- Binance run `binance-backfill-run-d928f6666827dd47` records 4,701 completed
  payload objects, 42,358,207,176 payload bytes, zero errors, and
  `payload_completion_ok = true`, but it is still raw staging and not a
  canonical NT catalog.
- OKX run `okx-3m-d812548c6c5871b5` records one strict daily tranche for
  2026-03-01 with 200 payload objects, 6,279,257,571 payload bytes, zero errors,
  and no selector-scope violations. The OKX manifest directory currently has
  116 manifest/progress objects, so acceptance must be ledgered per run/tranche,
  not inferred from prefix presence.
- Polymarket PMXT streaming still has physical data beyond accepted coverage:
  the prefix contains 915 objects and 344,758,798,407 bytes, while the inspected
  orphan acceptance manifest covers 149 objects and 57,936,847,067 bytes for
  2026-04-15 through 2026-04-23. Raw PMXT object presence is therefore physical
  evidence only until reconciled into accepted manifest coverage.
- Bybit has a large orphan acceptance manifest covering 30,710 objects and
  21,946,409,144 bytes with `source_proof_id = null`. That may be useful for
  reconciliation, but it is not acceptable as canonical BTE input until source
  proof binding, data families, gap policy, and normalization are validated.
- Hyperliquid core manifest listing is truncated at 1,000 keys, indicating a
  large manifest/progress surface that must be summarized through a ledger
  rather than manually inspected. Deribit remains high-risk because prior
  manifests recorded many provider-rate and invalid-instrument errors.

Distance from the overall backtesting engine:

1. Local single-object vertical slice: close. It has a tested accepted sample
   proof, generic run/proof/result boundaries, and NT execution through
   `BacktestNode`.
2. Backfill foundation: not complete. There is substantial staged raw data and
   a generic coverage-ledger/parser/aggregate plus local idempotent artifact
   writer, batch/local-file manifest-summary ingestion boundaries, a TOML
   coverage spec, an operator CLI for that spec, and generic TOML source-proof
   metadata binding. A real manifest-only ledger can now be generated, but the
   current S3 evidence produces a rejected ledger, not an accepted coverage
   ledger: all discovered canonical source-proof reports are pending, 24
   manifest files still need schema normalization, and 146 supported manifest
   records still lack source-proof binding. There are no accepted normalized row
   tables, no accepted instrument/gap policy ledger, and no NT catalog export
   from that data.
3. Production BTE: blocked by the backfill foundation. Running production BTE
   before the ledger/normalization/catalog gates would only prove the existing
   single-object sample path, not the overall research/backtesting platform.
4. Scalable new venue/data-family onboarding: partially shaped by the
   source-binding registry and typed run/proof boundaries, but not complete
   until the coverage ledger and normalization adapters are registry-driven and
   tested against more than the current sample.

Backfill must therefore be the next deliverable. The efficient order is:

1. Build a machine-readable coverage ledger from existing manifests and S3
   inventory only; no broad downloads.
2. Accept or explicitly reject the pending source-proof reports that can bind
   staged data; no raw staged data can become canonical BTE input while its
   source proof is pending.
3. Normalize the 24 unsupported manifest schemas into the generic coverage
   ledger or record them as unsupported-schema rejected coverage records.
4. Reconcile physical-only S3 objects, starting with PMXT and Bybit orphan
   acceptance manifests, into accepted or rejected coverage records.
5. Normalize one bounded accepted tranche into the declared table contract with
   source-proof and gap-policy checks.
6. Export an NT catalog from that normalized tranche and run BTE only after the
   catalog proof exists.

## Recommendation

Proceed with backfill-first proof, then production BTE proof; do not start
broad historical backfill or custom simulator work:

1. Add a coverage-ledger implementation that consumes manifest objects and S3
   inventory summaries without venue-specific code branches, records accepted,
   rejected, gap, and physical-only coverage, and fails on unbounded selectors
   before any download/conversion work.
2. Use the ledger to choose one bounded accepted tranche for normalization. Do
   not choose by venue name; choose by source-binding evidence state, completed
   manifest status, object count/byte budget, data family, and gap policy.
3. Normalize that tranche into the declared table contract, preserving
   source-proof id/version, source binding, raw object hash, byte count, table
   family, instrument universe metadata, and gap reason.
4. Export an NT catalog from the normalized tranche and prove that
   `BacktestNode` consumes it using NT's catalog/data config APIs.
5. Only after the backfill ledger, normalized rows, gap policy, and NT catalog
   proof exist should the branch create or provide SSM parameter paths for
   artifact-store S3 credentials and run `--publish-output
   --prove-published-catalog` under `nt-research-analytics/`.
6. Add real typed NT mappings for leverage maps, margin model, simulation
   modules, fill model, latency model, fee model, and settlement prices only
   when accepted source proof and result-contract claim limits justify each
   surface.
7. Keep unsupported NT venue/system model surfaces rejected before NT config
   construction; the declared venue placeholders must continue producing
   structured errors until real NT mappings land.
