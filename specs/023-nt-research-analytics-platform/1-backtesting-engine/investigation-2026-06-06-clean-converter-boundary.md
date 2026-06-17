# Backtesting Engine Investigation And Fix Report - 2026-06-06

## Status

This is not a completed production backtesting-engine rollout. It is a completed fix for the converter/output-boundary slice plus explicit primitive NT venue-control, catalog cloud-config mapping, typed unsupported NT data-query surface gating, TOML-owned raw-payload bounds, TOML-owned artifact-store config, SSM-backed artifact-store credential resolution, source-proof claim-limit propagation into generated result contracts, claim-limited `not_applicable` required-check handling, runtime source-bindings registry ownership for operator/report paths, report-only source-proof admissibility gating, and report-only legacy source-proof derivability gating.

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
- NT `BacktestDataConfig` `instrument_ids` and `OrderBookDelta` data-type mapping are now TDD-proven for the bounded manifest/data-config path; remaining query surfaces not yet supported by this slice (data-config `start_time`/`end_time`, `filter_expr`, `client_id`, metadata, `bar_spec`, `bar_types`, and `optimize_file_loading`) are declared in TOML schema, rejected with structured `UnsupportedNtSurface` errors before NT config construction, and recorded as unsupported resolved NT surfaces for result-contract claim limits
- artifact-root typed subpaths now resolve from the single configured root for `raw`, `nt-catalog`, `source-proofs`, `backtests`, `artifact-index`, and `research-analytics`; unsupported artifact-root schemes fail validation before a run
- Artifact Index record construction now has a pure contract helper for BTE-produced staged records: generated per-kind event/latest-pointer URIs under the single S3 `artifact_root`, `sha256` content-hash validation, required parent lineage refs, active lifecycle default, and producer-owned write-authority checks
- Artifact Index committed-discovery helpers now validate that generated latest pointers resolve only to matching active snapshots, reject stale or hash-invalid pointers, reject staged/orphan records as committed discovery truth, and model first-write/update pointer preconditions plus retry/rebase-required conditional failures
- Artifact Index cross-kind parent resolution now requires the child record's manifest lineage id and `sha256` hash to match the parent record, rejecting independently supplied latest-parent records with mismatched hashes
- Artifact Index event-create planning now models producer-owned immutable events: same URI plus same structured payload hash is idempotent, while a different payload at the same URI is rejected as an overwrite
- the pinned `object_store = 0.13.2` API exposes `PutMode::Create` and `PutMode::Update(UpdateVersion)` for optimistic-concurrency metadata commits; its AWS backend maps the canonical `conditional_put = "etag"` option to `If-None-Match: *` create-only writes and `If-Match: <etag>` updates, and the manifest now preserves that option while rejecting `conditional_put = "disabled"` for S3 artifact-store commits
- Artifact Index commit proof now has a bounded runner that writes immutable event, snapshot, and audit epoch objects, creates the first latest pointer with create-only semantics, updates the latest pointer with the observed prior ETag, rejects a stale-ETag update, reads back the pointer and snapshot, and resolves the committed snapshot using a computed canonical snapshot content hash. The real S3 proof used isolated root `s3://bolt-parquet/nt-research-analytics/artifact-index/proofs/backtesting-engine-006-artifact-index-20260608-3d6b1529`; report `/private/tmp/bte-artifact-index-proof-20260608-3d6b1529/output/artifact-index-commit-proof-report.json`; committed report file SHA256 `091501378ca70ca99b353d2252f040ef6b4d20c3d9b9e6db1dc29cc5d0489bf8`; `direct_s3_commit_proven = true`; `producer_iam_scope_proven = false`
- Artifact Index IAM-scope probing now attempts configured denied-kind event, snapshot, and latest-pointer create-only writes and records permission rejections versus successful unauthorized writes. The real S3 probe used isolated root `s3://bolt-parquet/nt-research-analytics/artifact-index/proofs/backtesting-engine-006-iam-scope-20260608-ca7445ca`; report `/private/tmp/bte-artifact-index-iam-proof-20260608-ca7445ca/output/artifact-index-commit-proof-report.json`; report content hash `5aabc1ab2280999f2c3dcac6e734308b06816e4bffd43057efd2a4d1d4339a82`; `producer_iam_scope_denied_write_attempts = 3`; `producer_iam_scope_denied_write_rejections = 0`; `producer_iam_scope_violation_count = 3`. This proves the current generic artifact-store SSM credential is not per-kind scoped.
- BTE-006 status is now recorded in `reference/artifact-index-commit-proof-status.backtesting-engine-006.2026-06-08.json`: create-only/conditional S3 commit mechanics are proven, and the 2026-06-15 scoped-producer proofs reject all 90 denied event, snapshot, and latest-pointer write attempts across the six current Artifact Index kinds with zero violations.
- AWS read-only inspection initially found only the broad artifact-store credential namespace. After explicit approval for the AWS security mutation, the Artifact Index producer namespace now has per-kind SSM `SecureString` parameter names for `raw`, `nt_catalog`, `source_proofs`, `backtests`, `artifact_index`, and `research_analytics`; credential values were not recorded. Repo-side policy generation remains in `artifact_index_iam_policy`, which generates per-kind S3 IAM policy JSON from configured `artifact_root`, `ArtifactKind`, and optional proof roots, with tests rejecting unrelated kind resources and `kind=*` wildcards.
- NT dependency selection is now machine-checked by `nt_dependency_proof`: it parses this BTE crate's embedded `Cargo.toml` and `Cargo.lock`, verifies every `nautilus-*` dependency uses the same git revision `6e059dcbb59ac1e582132fc431a581936c216c3c`, verifies Cargo.lock resolves those NT packages to the same revision, and verifies required feature enablement (`nautilus-backtest` has `examples` and `streaming`; `nautilus-persistence` has `cloud`).
- Artifact lifecycle config now rejects default delete/expiration rules, requires `active`/`archive`/`deep_archive` storage profiles, derives active-to-inactive state from a configured quiet window, and rejects committed discovery when the latest pointer or current snapshot metadata is not active/queryable
- S3 catalog storage options now fail before NT config construction if generic and Rust-specific maps are both set, or if an S3 option key is not supported by this pinned NT revision
- source-proof acceptance now enforces the schema rule that accepted canonical backfill input must use `directly_backfillable` or `owner_archive_backfillable`; bounded/current-only, pending, vendor/forward-capture-only, not-applicable, or excluded evidence states cannot become accepted BTE input
- source-proof acceptance now cross-checks registered TOML source-binding metadata for `product_family`, `table_family`, `evidence_state`, and `market_structure_fixture`; a proof cannot reuse a registered host/key while silently changing the data family, acceptance state, or market-structure fixture
- accepted source proofs now require a TOML-selected BTE market-structure fixture of `binary-option` or `perps-spot`; legacy `prediction-market`/`options`/`mixed` records remain parseable for non-current evidence reporting but cannot be accepted as canonical BTE input
- source-proof acceptance now rejects unknown `source_binding`/venue pairs before an accepted proof can be stamped; object selection keeps a defense-in-depth rejection for forged accepted records
- source-proof acceptance now requires `raw_sample_uri` and `schema_sample_uri` to be staged `s3://` artifact URIs, and accepted dataset selection requires the manifest object's `s3_uri` to exactly match the proof's `raw_sample_uri`
- accepted dataset selection now validates the staged object's `source_url` against the registered source-binding URI template path/query, not just the HTTPS host; a same-host monthly, aggTrades, or other data-family path cannot satisfy a daily trades binding
- source-proof acceptance now requires structured `acceptance_scope` facts (`planned_objects`, `completed_objects`, `failed_objects`, `skipped_objects`, `accepted_bytes`, and `selector_scope_violations`) instead of accepting prose-only completeness evidence; failed objects, selector-scope violations, inconsistent object accounting, skipped objects without a gap policy, and selected objects whose bytes exceed accepted bytes fail before canonical conversion
- non-L2 source-proof acceptance now requires structured `claim_limits` rows backing every `forbidden_claims` entry, so trade-replay or weaker data cannot rely on unstructured prose to block execution-quality, order-book, coverage, or fidelity claims
- source-proof required checks now support `not_applicable` only when a structured `claim_limits` row binds the same `evidence_ref`; this is generic schema behavior, not a Binance/Bybit branch
- `SourceProofReport` now carries first-class generic source-selection evidence before provider selection: `source_candidate_class`, `source_selection_status`, `usage_scope`, `official_free_gap_ref`, `paid_vendor_gap_ref`, `cost_ref`, `retention_freshness` and `cost` required checks, and thin `l2_replay_evidence` pointers. Paid/vendor candidates require a recorded official/free gap, forward-capture candidates also require paid/vendor gap evidence, non-selected candidates cannot be accepted for canonical backfill input, `one_off_backfill_data` cannot be accepted as canonical source-proof input, `L2_REPLAY` requires order-book delta or sufficient snapshot-cadence evidence, and pending/rejected reports cannot carry acceptance provenance. This closes `BACKTESTING_ENGINE-015` without adding venue/provider constants or storing heavy raw/catalog/result payloads in the proof.
- BTE-022 status is now recorded in `reference/source-proof-nt-catalog-mapping-status.backtesting-engine-022.2026-06-08.json`: pinned NT provides the required `ParquetDataCatalog`, `BacktestNode`, `OrderBookDelta`/`OrderBookDepth10`/`TradeTick`, and Polymarket instrument/parser surfaces; the isolated BTE slice now has TDD-proven manifest/data-config mapping for `OrderBookDelta` and configured `instrument_ids`, NT-native `BinaryOption`/`OrderBookDelta`/`TradeTick` catalog write/read/logical-hash coverage, NT-native BinaryOption L2 `BacktestNode` consumption with `BookType::L2_MBP` and exact OrderBookDelta iteration counts, a generic TOML/ledger-driven first-proof selector over configured event-family roles and row budgets, machine-checked `nautilus-polymarket` public provider/parser surface reuse, L2 result-contract validation for `event_count_ledger_hash` plus `selected_asset_ids_hash`, and generic runner input wiring that refuses L2Replay result construction without selector provenance. It still lacks PMXT raw-row-to-NT projection, instrument metadata binding, and BacktestNode proof over a selected source-backed PMXT catalog.
- BTE-022 source-catalog mapping readiness now requires explicit `nt_data_class_evidence_refs` for every required NT data class, so a new venue/data family cannot become ready by setting only `current_bte_status = "accepted"` and `parquet_catalog_status = "proven"`. This is generic: evidence refs live in the source mapping artifact, and the Rust gate checks data class names and proof refs without source or venue branches.
- PMXT Polymarket row-to-NT draft contract is now recorded in `reference/source-proof-pmxt-polymarket-row-to-nt-contract.2026-06-08.json`: `book`, `price_change`, `last_trade_price`, and `tick_size_change` rows line up with pinned NT Polymarket parser/data surfaces, but price-change implementation, mixed `timestamp_received` policy proof, selected trade-id policy tests, and tick-size-change catalog representation remain unresolved before even the bounded one-off selected-source catalog proof.
- PMXT Polymarket parser field-use audit is now recorded in
  `reference/source-proof-pmxt-polymarket-nt-parser-field-use.2026-06-08.json`:
  pinned NT declares `PolymarketQuote.hash`, but `parse_book_deltas` and
  `parse_quote_from_price_change` do not read it, and pinned NT tests construct
  price-change quotes with `hash: String::new()`. This resolves the quote-hash
  sub-blocker for the pinned NT revision only; dependency wiring, grouping,
  trade-id provenance, tick-size-change representation, catalog read-back, and
  `BacktestNode` consumption remain open.
- PMXT Polymarket trade-id policy status is now recorded in
  `reference/source-proof-pmxt-polymarket-trade-id-policy-status.2026-06-08.json`:
  pinned NT's websocket parser derives IDs because live `last_trade_price`
  events lack trade ids, while pinned NT's HTTP data API path already uses
  `transaction_hash` plus asset and sequence to disambiguate multi-fill
  collisions. The PMXT one-hour sample has `80,052` `last_trade_price` rows,
  zero blank transaction hashes, `80,010` distinct semantic trade events, zero
  cross-asset transaction-hash collisions, and `42` duplicate semantic trade
  groups that differ only by `timestamp_received`. The selected policy is to
  collapse those duplicate observations before assigning NT-style
  transaction_hash+asset+sequence trade IDs; semantically distinct same
  hash/asset fills still sequence using pinned NT's historical TradeId shape.
  TDD/catalog proof remains required before catalog acceptance.
- PMXT Polymarket price-change grouping status is now recorded in
  `reference/source-proof-pmxt-polymarket-price-change-grouping-status.2026-06-08.json`:
  pinned NT's live data client iterates `quotes.price_changes` and wraps each
  individual change into a single-change `PolymarketQuotes` payload before
  calling `parse_book_deltas`, even though the parser itself can parse a
  multi-change batch. Bounded PMXT sample evidence shows `timestamp_received`
  must be part of the source boundary: the first 10 seconds contain `184,734`
  price-change rows, `1,391` multi-row `(market, timestamp_received, timestamp,
  asset_id)` groups, and `16` source timestamp/asset groups that cross distinct
  receive batches. The preferred implementation policy is therefore
  one-row-to-one-NT-delta emission for pinned live-client parity, preserving
  timestamp_received as `ts_init`/coverage provenance; grouped parser output
  requires a separate proving test before use. A full-object grouping aggregate
  over the already-local 361 MB sample was intentionally stopped by evidence:
  DuckDB spilled a 487 MB temp file and failed with no local disk space, so full
  grouping scans are not acceptable as default source-proof workflow.
- PMXT Polymarket tick-size-change status is now recorded in
  `reference/source-proof-pmxt-polymarket-tick-size-change-status.2026-06-08.json`:
  pinned NT live handling rebuilds `BinaryOption` instruments on
  `tick_size_change`, emits `DataEvent::Instrument`, and starts a book epoch
  transition before deltas resume. The PMXT one-hour sample has `419`
  tick-size-change rows across `343` assets, all `0.0100 -> 0.0010`, with `76`
  assets carrying duplicate same-transition rows. NT catalog can store multiple
  `InstrumentAny` snapshots, but `BacktestNode` loads instruments up front and
  standard `BacktestDataConfig` catalog replay feeds `Vec<Data>` through
  `BacktestEngine::add_data`; NT's `Data` enum has no `InstrumentAny` variant.
  NT live-style `DataEngine::process(&InstrumentAny)` exists, but that is a
  separate path from catalog-backed replay. A focused pinned-NT source recheck
  on 2026-06-08 proves standard `BacktestNode` catalog replay does not expose a
  timed `InstrumentAny` stream; multiple up-front snapshots collapse through
  instrument-id keyed cache/exchange insertion rather than scheduled replay. A first one-object proof may
  exclude tick-changing assets only with explicit claim limits; full L2
  acceptance must either accept a no-tick-size-change source universe or prove a
  separate timed instrument-epoch replay path.
- PMXT Polymarket first-proof universe policy is now recorded in
  `reference/source-proof-pmxt-polymarket-first-proof-universe-policy.2026-06-08.json`:
  the same one-hour object has `71,593` assets, `343` assets with
  `tick_size_change`, and `823` assets with `book`, `price_change`, and
  `last_trade_price` rows but no tick-size change. Eligible no-tick assets have
  a median `749` replay rows, with `451` eligible assets at or below `1,000`
  rows, so a bounded first L2 proof can select by a TOML/source-proof-owned
  predicate instead of hardcoded asset IDs. This path can only claim
  catalog/read-back/`BacktestNode` plumbing for the selected unaffected
  universe; it still cannot claim dynamic tick-size replay, full PMXT
  Polymarket L2 acceptance, or broad backfill.
- PMXT selected-source staging is now recorded in
  `reference/source-proof-pmxt-selected-source-slice.2026-06-08.json`: the
  generic `selected_source_slice` CLI used the concrete selector report to
  materialize `/private/tmp/bte-pmxt-selected-source-slice-2026-06-08/selected-source.parquet`
  from `/private/tmp/polymarket-may20-one.parquet`. The report binds the source
  parquet SHA256 `0de44455fde7aedd6678fa30cc1ef86ba215eaf70fb3f7b9735510e1371f6567`,
  selector report SHA256
  `647f0cee89becb46b7051992dd7fea25ca08be3b551bd6873cd21be2ebd7b524`,
  output parquet SHA256
  `a0890b3d87b913010325a7e1c4988bfab0654d3fe532ea5caa94494b6582e79e`,
  `64,877,467` scanned source rows, `4` selected rows, one selected asset, and
  `selected_asset_ids_hash`
  `edc5e3c70031056cf544d2cf581c5fe2ee3122886090ae513d6321a34c99d966`.
  This is a one-off proof staging artifact only with `usage_scope =
  one_off_backfill_data`. The Rust slicer took `218.96s` wall time over the
  one-hour sample, so it must not become a broad backfill method. Broad PMXT
  history is out of scope unless a separate source-proofed plan is explicitly
  authorized.
- PMXT selected-token Polymarket metadata gating is now recorded in
  `reference/source-proof-pmxt-selected-polymarket-metadata-gate.2026-06-08.json`:
  the selected PMXT row carries token
  `101573629105061692824394189329292260077476973116785474086922405861943493792845`
  and condition
  `0xd7c7d829b33a3ad4698fc13b77c960c68aa3e05d03683b173e0af9db6c1c555c`.
  Official CLOB endpoints bind that token to a sibling token, outcomes, tick
  size, and minimum order size, but they do not return an NT `GammaMarket`.
  Gamma `/markets` probes by `clob_token_ids` and `condition_ids` returned an
  empty array, camel/singular filter probes returned default pages without the
  selected token/condition, and unauthenticated Gamma search returned 401. The
  new generic `polymarket_metadata_gate` deserializes source-backed Gamma JSON
  into NT's `GammaMarket` and calls NT's `parse_gamma_market`; for this selected
  PMXT proof it reports `blocked_missing_gamma_market`. Therefore the next
  allowed step is to acquire or stage source-backed GammaMarket metadata for
  the selected condition/token; synthesizing `BinaryOption` metadata from PMXT
  rows or CLOB-only abbreviated fields is explicitly forbidden.
- Current BTE implementation audit after selecting that policy: the source-proof
  and claim-limit governance pieces exist, and the isolated crate now has
  TDD-proven manifest/data-config support for `OrderBookDelta`, configured
  `instrument_ids`, and `OrderBookDelta`/`L2_REPLAY` fidelity binding. It still
  lacks source-backed PMXT row-to-NT projection and catalog projection,
  while `catalog_projection.rs` now has NT-native logical hash/read-back coverage
  for `BinaryOption`, `OrderBookDelta`, and `TradeTick` fixture data, and
  `backtesting_vertical_slice_catalog_and_node.rs` now proves an NT-native
  BinaryOption L2 `OrderBookDelta` catalog is consumed by BacktestNode with
  exactly two iterations under `BookType::L2_MBP`. The new
  `first_proof_selector` path is generic and TOML/event-ledger driven: required
  event families, excluded event families, row budget, and max selected assets
  come from the spec, and the report binds `event_count_ledger_hash` plus
  `selected_asset_ids_hash`. `BacktestRunInputs` now has optional generic
  selector provenance, and L2 replay run-contract construction rejects missing
  selector hashes before stamping a result contract. The next implementation
  gate is therefore PMXT source-backed binary-option L2 catalog projection plus
  BacktestNode proof using NT classes, not a venue-specific branch.
- Pinned NT Polymarket API exposure is now recorded in the PMXT row-to-NT
  contract and BTE-022 mapping status. The required instrument/provider,
  Gamma parse, BinaryOption build/rebuild, websocket message, snapshot,
  price-change, and trade parser surfaces are public once the isolated BTE
  crate depends on `nautilus-polymarket`; the exact historical
  `build_polymarket_trade_id` helper is `pub(crate)`, so BTE must either
  mirror the pinned helper format with TDD/provenance or wait for an upstream
  public API before claiming direct helper reuse.
- The bounded-first-proof go/no-go checkpoint is now recorded in
  `reference/source-proof-bte-bounded-first-proof-go-no-go.2026-06-08.json`:
  go for TDD only on the selected no-tick PMXT Polymarket first proof, no-go
  for production backfill or broad PMXT L2 backfill, and no-go for dynamic
  tick-size replay until BacktestNode catalog epoch handling is proven.
- generated result contracts now preserve structured source-proof claim-limit evidence from the accepted proof instead of rebuilding source limits from plain `forbidden_claims` strings
- non-latest source-proof pins now require structured manifest justification: `normal` runs still cannot pin them, non-normal pins require `proof_pin_reason_code`, and `audit_or_investigation` pins require `proof_pin_reason_detail`
- the accepted `proof_pin_reason_code` vocabulary now matches the plan/reference contract, including published-result reproduction and regression-comparison pins
- the CLI has an explicit `--publish-output` opt-in that copies the verified local artifact tree to `manifest.output_prefix` through NT/object-store plumbing after the local run succeeds
- published artifacts are create-only: the operator preflights the bounded target artifact set and writes through object-store `PutMode::Create`, so an existing published artifact rejects the run instead of being overwritten
- publish flows resolve and validate artifact-store options before reading the accepted object, so missing S3/SSM setup cannot waste local object I/O on large accepted objects
- backfill preflight selection is now a cheap TOML-driven gate over the coverage ledger: it selects at most one bounded canonical-ready accepted tranche before any payload download, conversion, catalog projection, or backtest work can start
- source-proof migration preflight is now a cheap TOML-driven gate over the legacy derivability report: requested table families, required derivable fields, raw-payload count, S3-bound status, and byte budget are config-owned, so candidate selection does not depend on venue names or provider-specific branches
- combined backfill readiness is now a cheap TOML-driven gate over the backfill preflight report and the source-proof migration preflight report, so a path is not considered ready unless both the canonical-ready tranche and required source-proof candidate are selected
- source-binding coverage is now a cheap TOML-driven gate over the configured source-binding registry and coverage ledger; it reports whether required table-family bindings have ledger records without inferring from prefixes or venue names
- operator source-proof acceptance, accepted-dataset selection, source-proof admissibility reports, and source-proof scope reports now load the source-bindings registry from spec-owned `source_bindings_path`; the committed registry is no longer the runtime source of truth for those TOML-driven paths
- source-proof scope coverage is now a cheap object-level gate over one accepted proof and one manifest; it proves whether the proof's raw sample object exists inside a manifest and whether the enclosing manifest is broader than the accepted proof scope
- accepted-tranche manifests are now a cheap TOML-driven artifact boundary over source-proof scope reports; the first accepted reference trades tranche is exactly one object and is hash-bound to the source-proof scope report that selected it
- backfill execution plans are now a cheap TOML-driven artifact boundary between an accepted tranche and an operator run-spec; they refuse to expose any payload object for download unless source proof id/version, source binding, raw sample URI/hash, accepted object URI/source URL/hash/bytes/date, and object byte budget all match, and they bind the accepted-tranche manifest hash plus the submitted run-spec hash
- the main BTE operator CLI now requires a `--execution-plan` artifact and rejects missing plans, plan/run-spec/object mismatches, and plan byte-budget violations before reading the local accepted object, closing the manual gap between plan generation and operator invocation
- the first accepted reference trades tranche has been driven through the existing operator path, not a new converter path: verified raw object -> existing native-trades converter -> canonical Parquet -> NT `ParquetDataCatalog` projection -> NT catalog read-back -> NT `BacktestNode` run -> result contract
- artifact-store options are TOML-owned; raw S3 credentials in TOML are rejected; `s3://` publish/proof requires `[manifest.artifact_store.ssm_parameters]` to resolve `access_key_id` and `secret_access_key` through the Rust AWS SDK before any backtest or object-store operation starts
- the current `BacktestExtensionSurface` classification is recorded in `backtest-extension-surface-matrix.md`; supported primitive NT controls are TOML pass-throughs, Bolt-owned pieces are provenance/governance boundaries, unmodeled NT model/system surfaces fail before NT config construction, and each successful run now writes `backtest-run-manifest.json` plus result-contract claim-limit entries for resolved NT defaults, supported run/venue/catalog pass-through fields, and unsupported NT surfaces

No-go for broader production claims:

- `s3://bolt-parquet/nt-research-analytics/` now contains SSM-backed staging proof objects plus bounded reference backtest publications. The latest recursive listing returned 37 objects totaling 12,067,580 bytes: the Binance daily raw ZIP; scratch and accepted-candidate Binance source-proof evidence under `source-proofs/`; 10 Bybit sample/reference catalog/result artifacts under `backtests/backtesting-vertical-slice-bnbusdc-2026-03-01/`; 10 corrected accepted Binance BNBUSDC catalog/result artifacts under `backtests/backtesting-vertical-slice-binance-bnbusdc-2026-03-01-accepted/`; and 10 superseded Binance publication artifacts under `backtests/backtesting-vertical-slice-binance-bnbusdc-2026-03-01/` retained as forensic evidence only because its published accepted-source-proof carried stale scratch license-check evidence.
- the user confirmed converted artifacts were intentionally deleted during this investigation, so any missing historical converted output is expected current state, not accepted output evidence
- two real S3 writes have now been performed through the main `--publish-output --prove-published-catalog` catalog/result path: the Bybit sample/reference tranche and the accepted Binance BNBUSDC daily tranche. This proves the clean S3 path for those bounded slices only, not broad historical backfill.
- artifact-store SSM parameter paths now exist at `/bolt/artifact-store/s3/access-key-id` and `/bolt/artifact-store/s3/secret-access-key` in `us-east-1`; secret values were not printed or written to TOML, and the paths must remain the only runtime credential references
- the Bybit sample/reference S3 publish/proof run stamps `direct_s3_catalog_access_proven = true` in `published-catalog-proof.json` with `nt_iterations = 937` and `expected_iterations = 937`; the accepted Binance daily run stamps the same proof fields with `nt_iterations = 71431` and `expected_iterations = 71431`. Earlier local-only proofs remain forensic evidence only.
- the accepted-tranche operator proof now proves deterministic local conversion/catalog/backtest behavior plus direct production S3 catalog readback for one 8.5 KB Bybit sample/reference object and one 1,066,394-byte Binance daily object. It does not prove broad historical backfill or multi-day/multi-instrument coverage.
- the direct S3 publish/proof command with only `region` configured fails fast with `artifact_store.ssm_parameters must resolve access_key_id and secret_access_key before publishing to an s3 output_prefix`, and leaves no output directory; the corrected next proof must use the configured SSM parameter paths
- only two BNBUSDC 2026-03-01 trade-replay objects are proven in this slice: the original Bybit sample/reference object and the accepted Binance daily object. Bybit and Binance are proof/config instances, not production converter special cases.
- generic `run_manifest` unit fixtures now use synthetic accepted dataset values rather than duplicating the accepted Bybit/BNBUSDC sample proof; the committed sample proof/run-spec and end-to-end sample fixtures remain the authoritative BNBUSDC evidence
- the registry now carries a second native-trades source-binding candidate (`binance-spot-native-trades`) so the binding gate is not single-venue, and the converter can process its headerless single-member ZIP shape. Exactly one Binance daily trade object is accepted BTE input after staged S3 raw/schema/license/current-instrument evidence, claim-limited `not_applicable` instrument-universe policy, source-proof scope, accepted tranche, execution plan, and direct S3 catalog proof all passed.
- Task ledger reconciliation now marks `BACKTESTING_ENGINE-004`, `BACKTESTING_ENGINE-005`, `BACKTESTING_ENGINE-007`, `BACKTESTING_ENGINE-009`, `BACKTESTING_ENGINE-035`, and `BACKTESTING_ENGINE-036` complete based on the spec/plan definitions, manifest/output-prefix tests, direct-S3 published-catalog proof artifacts, the small multi-instrument S3 `ParquetDataCatalog` proof, manifest-to-NT mapping tests, and Artifact Index contract tests.
- `BACKTESTING_ENGINE-007` proof evidence: the new `nt_catalog_proof` binary ran with a TOML-owned fixture at `/private/tmp/bte-nt-catalog-proof-20260608/nt-catalog-proof-s3.toml`, resolved artifact-store S3 options through the existing Rust SSM path, refused non-empty catalog roots before NT writes, wrote two configured instruments (`BTCUSDT.SIM`, `ETHUSDT.SIM`) and six synthetic `TradeTick` rows to `s3://bolt-parquet/nt-research-analytics/nt-catalog/proofs/backtesting-engine-007-multi-instrument-20260608-2be6f865`, queried them back through NT `ParquetDataCatalog`, and ran NT `BacktestNode` directly against the same S3 catalog. The report at `/private/tmp/bte-nt-catalog-proof-20260608/output/nt-catalog-proof-report.json` has content hash `035774bed9a46d70ae4d70da30391c6ef49357f16e4cc7b09ab2a7712ee238ee`, `nt_instrument_count = 2`, `nt_trade_ticks = 6`, `nt_backtest_iterations = 6`, and `direct_s3_catalog_access_proven = true`.
- complex NT model surfaces are now manifest-declared but not mapped into NT: leverage maps, margin model, simulation modules, fill model, latency model, fee model, and settlement prices fail with structured `UnsupportedNtSurface` errors before NT config construction.
- compatible CSV native-trade spot venues can be added through source proof plus run-spec TOML mapping, including `csv_gzip`, `csv_text`, configured single-member `single_csv_zip` containers, headered or headerless schemas, timestamp units, and side-token mapping; new NT data classes or instrument families beyond the current `TradeTick`/`CurrencyPair` path are not yet TOML-only and must fail fast until a typed NT projection path is added
- no execution-quality, queue-position, order-book-liquidity, multi-day, or multi-instrument claim is supported by this slice

## Current Source Facts

- Source-facts recheck on 2026-06-07 was collected at then-HEAD `0427c128bf158be721cae32721189966ab0bbacc`: branch `codex/bte-clean-converter-nt-use-continue`, `main = origin/main = c1b1f7b49414008a11af11da24ebc49762debf54`, and `git status --short --branch` reported a clean branch. `git merge-base main HEAD` returned `ca40e51792e11cc3dd954c9fe8321c7186a25e58`; `git rev-list --left-right --count main...HEAD` returned `19 110`, so this investigation branch is now divergent from current `main` and any final implementation or PR claim requires an explicit main reconciliation step.
- Read-only reconciliation-risk recheck at branch head `fbec78d89290c30e847a0ebf2afc0a46e23a3a77` found `63` branch-changed files and `108` current-main-changed files since merge base, with `comm -12` returning no overlapping filenames. `git merge-tree --write-tree HEAD main` succeeded and returned merged tree `1d28eb375ee1c59db1d86154c405ad7c7bdf5955`, so the current evidence shows no textual merge conflicts. This does not replace an actual main reconciliation plus focused verification before any final PR/completion claim.
- Actual main reconciliation was then performed on successor branch `codex/bte-clean-converter-nt-use-main-reconcile`, created from current `main` at `c1b1f7b49414008a11af11da24ebc49762debf54`; merge commit `183d1980` merged `codex/bte-clean-converter-nt-use-continue` without textual conflicts. Focused verification on the reconciled branch passed: `cargo fmt --check`, `git diff --check`, `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- clippy --locked --all-targets -- -D warnings`, and `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked`. The only reconciliation fixes required were a missing `source_bindings_path` field in a source-proof-admissibility test fixture, clippy cleanup in tests/helpers, and moving helper functions before the test module in `runner.rs`.
- Relevant local/remote BTE branches visible from this worktree include `chore/bte-runtime-hardening`, `codex/backtesting-engine-increment`, `codex/backtesting-vertical-slice`, `codex/bte-clean-converter-nt-use`, `codex/bte-clean-converter-nt-use-continue`, `codex/bte-e037-trace-row`, and `origin/codex/bte-e037-trace-row`; stale branches are forensic references only and are not implementation sources for this branch.
- Open PR recheck through the GitHub connector on 2026-06-07 returned open PR `#592` (`codex/bte-e037-trace-row`) for BTE/backtesting/converter queries, open docs PR `#576` (`docs/438-normalization-catalog-plan`) for the BTE query, and draft proof PR `#496` (`feat/438-bte-gate4-run-proof`) for BTE/backtesting queries; none of these PRs proves this branch's clean production artifact path.
- Accepted raw object exists: `s3://bolt-parquet/backfill-staging/2026-06-01/bybit/raw/v1/source=public_archive/family=tick_trades/category=spot/dt=2026-03-01/symbol=BNBUSDC/object=d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598.csv.gz`
- S3 listing result for that object: one object, `8505` bytes
- Fresh read-only S3 `head-object` after the CLI preflight fix confirmed the accepted raw object still exists with `ContentLength = 8505` and ETag `"3959bd2c4ff9ac093c7692b812cea2f8"`; later, only this single approved 8505-byte object was downloaded to `/private/tmp/bte-bnbusdc-current-schema-object.csv.gz` for the current-schema local proof
- Accepted source proof is committed at `specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.bnbusdc-2026-03-01.json`
- Run spec binds the same object, hash, output prefix, and trade-replay claim limits in `specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml`
- Run spec now configures `[manifest.artifact_store].rust_storage_options = { region = "us-east-1", conditional_put = "etag" }` so the S3 artifact-store path is explicit about object_store conditional-write semantics instead of relying on a hidden default
- Run spec binds `[converter] identity = "csv-native-trades-to-canonical-trades.v1"`, `version = "1"`, `[converter.raw_payload] container = "csv_gzip"`, `max_object_bytes = 8505`, `max_decoded_bytes = 1048576`, and `[converter.csv] has_headers = true` plus column/timestamp/side-token mapping. The Bybit-specific values live in the sample source proof and run-spec data, not in operator/runner control flow.
- Source-binding registry coverage is no longer single-venue for native trades: `bybit-spot-tick-trades` and `binance-spot-native-trades` are both configured as backfillable `native-trades`/`trades` bindings. The Binance row points at Binance Data Vision spot daily `trades` zip files and remains a candidate only; it does not create an accepted proof or bypass object/sample/hash gates.
- Converter/source-binding extensibility audit: source bindings are committed TOML registry data, and exact searches showed registry `extractor` strings are not executed by the BTE Rust path; source-proof acceptance only uses configured binding metadata for venue/product/evidence/table-family and source URL template checks. Adding a venue that emits the existing CSV native-trades shape is therefore registry/run-spec work: add the source-binding row, source proof, accepted object proof, and `[converter.csv]` mapping. Adding a genuinely new raw payload shape or NT data family is still compiled adapter work by design: `canonical_trades.rs` currently registers one generic `csv-native-trades-to-canonical-trades.v1` converter mapped to NT `TradeTick`. The next generalization should be data-family based, not venue based.
- Runtime source-bindings registry audit: `backfill_binding_coverage.rs` already read the configured registry from TOML `source_bindings_path`, but source-proof acceptance and object selection still used the embedded committed registry. That created a dual path where a report could validate one registry while the operator accepted against another. The operator run-spec, source-proof admissibility spec, and source-proof scope spec now carry `source_bindings_path`, and those runtime paths call the same resolver before acceptance or object selection.
- Bounded Binance candidate evidence collected without broad backfill: `https://data.binance.vision/data/spot/daily/trades/BNBUSDC/BNBUSDC-trades-2026-03-01.zip` returned `200`, `content-length = 1066394`, one CSV member `BNBUSDC-trades-2026-03-01.csv`, decoded length `5287070`, row count `71431`, and ZIP SHA256 `433d32b8d828abee5e1937e01372d16f7edadc14c41fe736b0b9577541fa5e81`, matching the Binance `.CHECKSUM` sidecar. The CSV is headerless with columns matching Binance native-trades semantics (`trade_id`, `price`, `qty`, `quote_qty`, `time`, `is_buyer_maker`, `is_best_match`) and microsecond timestamps. This was adapter evidence before acceptance; it became BTE input only after the later S3 raw/evidence staging, source-proof admissibility, source-proof scope, accepted tranche, execution plan, and direct S3 catalog proof all passed for exactly one daily object.
- Official Binance public-data documentation checked on 2026-06-07 confirms the archive is public market data exposed through Binance Data Collection, split into `daily` and `monthly` files, with new daily data available the next day, spot timestamps in microseconds from 2025-01-01 onward, `trades` files sourced from `/api/v3/historicalTrades`, and `.CHECKSUM` sidecars for each ZIP (`https://github.com/binance/binance-public-data`). The same README lists `Licence: MIT`; the corrected accepted Binance proof stores that README as the bounded public-data license evidence for this single daily object, with SHA256 and S3 evidence URI bound in the source proof. No separate private approval artifact is required for this bounded public-data proof.
- Read-only fetch of the existing one-off Binance staging manifest from `s3://bolt-parquet/backfill-staging/2026-06-01/binance/manifests/v1/run=binance-backfill-run-d928f6666827dd47/binance-backfill-manifest.json` produced `4701` payload records, `11600667` bytes, and SHA256 `b37d01f30932c8af4d8b9bc112b031569afbdf3a2db3756ec1f2a79626880c36`. It contains no `spot`/`daily`/`trades`/`BNBUSDC` object for `2026-03-01`; the staged BNBUSDC trade object is monthly March 2026 at `s3://bolt-parquet/backfill-staging/2026-06-01/binance/raw/v1/source=data.binance.vision/product=spot/frequency=monthly/family=trades/symbol=BNBUSDC/dt=2026-03/object=9fcdae9872ab3c7ff8f13d5f3c1830b017b25561fe6140493decfa079ee56aa6.zip`, so it cannot satisfy the configured daily source binding.
- Fresh recheck on 2026-06-07 kept that conclusion for the broad pre-existing `backfill-staging/2026-06-01/binance` manifest: `curl -sS -I https://data.binance.vision/data/spot/daily/trades/BNBUSDC/BNBUSDC-trades-2026-03-01.zip` returned HTTP 200, `content-length = 1066394`, and `last-modified = Mon, 02 Mar 2026 01:44:18 GMT`; the `.CHECKSUM` sidecar returned `433d32b8d828abee5e1937e01372d16f7edadc14c41fe736b0b9577541fa5e81  BNBUSDC-trades-2026-03-01.zip`. The expected daily key under that old broad staging prefix returned S3 `HeadObject` 404, so the old broad staging manifest cannot satisfy the configured daily binding. The later accepted Binance proof deliberately used the new single-object staging path under `s3://bolt-parquet/nt-research-analytics/raw/`, not that broad prefix.
- Bounded local inspection of the daily Binance ZIP downloaded only the 1,066,394-byte public object to `/private/tmp/bte-binance-bnbusdc-2026-03-01.zip`; SHA256 matched the official checksum, `unzip -l` showed one member `BNBUSDC-trades-2026-03-01.csv` with decoded length 5,287,070 bytes, `wc -l` returned 71,431 rows, the first/last trade timestamps were `1772323201711256` and `1772409599584254` microseconds (`2026-03-01T00:00:01Z` through `2026-03-01T23:59:59Z`), and the first-row SHA256 was `337ce74e0eba11abdb66d987f7687d117cd1c2ec17cda39e3ee7ba2675c9ca64`. Fresh focused verification `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test run_from_run_spec_uses_configured_single_csv_zip_payload -- --nocapture` exited 0; the existing test proves the operator can process a headerless single-member Binance ZIP with microsecond timestamps through NT for the `binance-spot-native-trades` binding. Therefore no venue-specific converter code was needed for this candidate; the later accepted-proof staging/governance gates closed the remaining blocker for one bounded daily object.
- Official Binance Spot API docs identify `/api/v3/exchangeInfo` as exchange information and define symbol filters such as `PRICE_FILTER`, `LOT_SIZE`, and `NOTIONAL` as trading-rule metadata. A bounded current read on 2026-06-07 using `https://api.binance.com/api/v3/exchangeInfo?symbol=BNBUSDC` returned `status = TRADING`, `baseAsset = BNB`, `quoteAsset = USDC`, `tickSize = 0.01000000`, `stepSize = 0.00100000`, and `minNotional = 5.00000000`. This is useful metadata for an instrument fixture, but it is current endpoint evidence, not a dated historical universe snapshot for 2026-03-01; it therefore does not pass the source-proof `instrument_universe` check for the March 2026 backfill object.
- Historical Binance `exchangeInfo` public-data root-cause check: the official Binance public-data README documents daily/monthly public market data for spot `aggTrades`, `klines`, and `trades`, with checksums and MIT license, but it does not document dated `exchangeInfo` or symbol-filter snapshots (`https://github.com/binance/binance-public-data`). A Binance Developer Community answer for historical `exchangeInfo` snapshots says they are not believed to be available and recommends storing symbol info locally for later queries (`https://dev.binance.vision/t/historical-snapshots-of-exchangeinfo-endpoint/12905`). Direct HEAD probes on 2026-06-07 for likely Data Vision paths all returned HTTP 404: `data/spot/daily/exchangeInfo/BNBUSDC/BNBUSDC-exchangeInfo-2026-03-01.zip`, its `.CHECKSUM`, `data/spot/daily/exchangeInfo/exchangeInfo-2026-03-01.zip`, its `.CHECKSUM`, `data/spot/daily/metadata/BNBUSDC/BNBUSDC-metadata-2026-03-01.zip`, and `data/spot/daily/symbols/BNBUSDC/BNBUSDC-symbols-2026-03-01.zip`. Therefore the preferred historical-instrument solution is not likely to come from public Data Vision; it needs a Binance/BD-provided dated snapshot or an explicit narrower source-proof policy.
- Binance instrument-universe solution path is now explicit: the preferred proof is a Binance/BD-provided dated `exchangeInfo` or equivalent symbol-metadata snapshot covering BNBUSDC on 2026-03-01, stored and hashed as the proof's `instrument_universe` evidence. For the bounded accepted daily proof, the current contract uses the narrower single-symbol `TRADE_REPLAY` policy by marking `instrument_universe` as `not_applicable` only because a matching blocking claim-limit row uses the same `evidence_ref` and forbids historical venue-rule, fillability, rounding, sizing, or execution-quality claims. That narrower policy is explicit proof data and must not be silently inferred from current `exchangeInfo` for future sources.
- Source-proof contract support for the narrower path was added test-first: `not_applicable_required_check_requires_matching_claim_limit_evidence` failed RED before `CheckOutcome::NotApplicable` existed, then passed after acceptance began rejecting `not_applicable` checks without a matching `claim_limits.evidence_ref`. Verification on the reconciled branch passed `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked not_applicable_required_check_requires_matching_claim_limit_evidence -- --nocapture`, `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked source_proof -- --nocapture`, `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- fmt --check`, and `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- clippy --locked --lib -- -D warnings`.
- A scratch Binance daily source-proof candidate was written only under `/private/tmp` and checked with the existing admissibility CLI. The report at `/private/tmp/bte-binance-daily-source-proof-admissibility-output/source-proof-admissibility-report.json` has content hash `b559d7d93a993cbd4fc276b2e23c4c61e2410979d63759643b480c35b37a384e`, file SHA256 `daf612393493011c19673f19607e1510f664184f70155c3f03afab62cb34741c`, one current-contract record, zero accept-ready records, and `current_contract_rejected` with `acceptance_error = "unmet required checks: license, instrument_universe, storage"`. This is retained as rejection evidence only; the accepted Binance proof later replaced it with S3-staged raw/schema/license/current-instrument evidence and the claim-limited `not_applicable` instrument-universe policy.
- A bounded local scratch Binance execution proof then used the same public daily ZIP, an execution plan with exactly one object, and only `/private/tmp` outputs: `/private/tmp/bte-binance-local-proof-20260607/plan2/backfill-execution-plan.json` SHA256 `9e44deece2a364583bbaf803e1793d979fce6d90826ae2d6ffab486b6e5feedc` selected the single 1,066,394-byte daily object; the first attempted run failed before raw-object reading because the scratch `schema_sample_uri` was not staged under `s3://`, and regenerating over the changed plan path failed with the expected dirty-artifact refusal. A fresh plan/output directory then ran successfully with `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin backtesting-vertical-slice -- --run-spec /private/tmp/bte-binance-local-proof-20260607/run.toml --execution-plan /private/tmp/bte-binance-local-proof-20260607/plan2/backfill-execution-plan.json --object /private/tmp/bte-binance-bnbusdc-2026-03-01.zip --output-dir /private/tmp/bte-binance-local-proof-20260607/output2`. Result-contract SHA256 was `ba16c52ce1315331e8146bcd4a8feab64a6518201065dbb793d4536d13e8ad4d`, accepted-source-proof SHA256 was `8fbb2fdc46772e47854fd7dcf8641de8cb39b2f507a53db201bdb3228d637787`, conversion-manifest SHA256 was `a2fc20149546dd37bbb0c8c3f907877244e129005382581bad97f2e579f107bd`, canonical parquet SHA256 was `fa97961e8a183157a9a9d8fb821060e86c9c7f5c17e170bb54c969846b43f2d9`, and the output directory was only 5.2 MB. The run produced 71,431 canonical rows, NT instrument `BNBUSDC.BINANCE`, catalog hash `8c128fe5acbb2e0df7c0f9b30d80de16acb285ca95f67a7bfc08c969f6b48362`, pinned NT version `6e059dcbb59ac1e582132fc431a581936c216c3c`, NT `BacktestNode` iterations `71431`, backtest start/end `1772323201711256000`/`1772409599584254000`, and zero orders/positions as expected for the no-op strategy. This is technical local evidence that the generic CSV native-trades adapter plus NT catalog/readback/backtest path works efficiently for the Binance daily shape; it is superseded for acceptance by the later S3-staged accepted proof and corrected direct-S3 publication.
- A generic single-object raw staging ingress was added after that proof so the next backfill attempt does not repeat the previous broad/slow manual staging pattern. `backfill_object_staging` verifies a configured local payload's byte count and SHA256, writes exactly one configured object URI with `object_store` create-only semantics, enforces `<artifact_root>/raw/` placement, reuses the same SSM-backed artifact-store option resolver as result publishing, and emits a `payload_records` manifest shape consumed by the existing source-proof scope gate. It is venue/data agnostic: the spec owns `source_url`, `output_object_uri`, `archive_date`, and `schema_columns`; the module has no Binance/Bybit constants. Focused TDD evidence: `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_object_staging -- --nocapture` first failed RED on missing module, then passed `3` tests after implementation; `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked artifact_store -- --nocapture` passed the existing artifact-store resolver coverage; `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- clippy --locked --all-targets -- -D warnings` passed. Local smoke proof with the real Binance daily ZIP also passed: `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin backfill_object_staging -- --spec /private/tmp/bte-binance-local-proof-20260607/backfill-object-staging-spec.toml` wrote `/private/tmp/bte-binance-local-proof-20260607/staging-manifest/backfill-object-staging-manifest.json`, manifest content hash `671ec06de57975dc736da5478596a09803960915fe9ceeb768ab732feb8c7ea2`, manifest file SHA256 `43a575272b0111b54029d787a340c5822d79d1fc41b989e4bbbb96965367a52e`, one `payload_records` entry, and one local staged object whose SHA256 remained `433d32b8d828abee5e1937e01372d16f7edadc14c41fe736b0b9577541fa5e81`.
- A generic source-proof evidence staging ingress was then added for the small non-raw artifacts that an accepted proof must reference: schema samples, license/approval evidence, retention notes, and instrument-universe snapshots. `source_proof_evidence_staging` verifies every configured local evidence file's byte count and SHA256, writes each configured URI with `object_store` create-only semantics, enforces `<artifact_root>/source-proofs/` placement, reuses the same SSM-backed artifact-store option resolver, and emits `evidence_records` containing `evidence_kind`, URI, SHA256, and bytes. It is venue/data agnostic: the spec owns the evidence kinds and URIs, and the module has no Binance/Bybit constants. Focused TDD evidence: `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_source_proof_evidence_staging -- --nocapture` first failed RED on missing module, then passed `4` tests after implementation; `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- clippy --locked --all-targets -- -D warnings` passed. Local Binance-shaped smoke proof also passed: `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin source_proof_evidence_staging -- --spec /private/tmp/bte-binance-local-proof-20260607/source-proof-evidence-staging-spec.toml` wrote `/private/tmp/bte-binance-local-proof-20260607/evidence-staging-manifest/source-proof-evidence-staging-manifest.json`, manifest content hash `529e26f151adbdd3cab22ea4625d31fd32f36d6797f2ae5ee83f4aab27d6341f`, manifest file SHA256 `adb877c8df40c36919b70089c3bfa17b3ae18c88683c74dd71a0eda75d042e76`, three records totaling 1,005 bytes, and local `schema_sample`, `license`, and `instrument_universe` evidence files whose staged SHA256 values matched the spec. That smoke proof is forensic only; the accepted Binance proof later replaced it with the S3-staged official README license evidence, schema sample, and claim-limited current-instrument evidence set under proof id `source-proof-binance-spot-native-trades-bnbusdc-2026-03-01`.
- After creating SSM SecureString artifact-store parameters in `us-east-1` at `/bolt/artifact-store/s3/access-key-id` and `/bolt/artifact-store/s3/secret-access-key` without printing values, the real S3 raw-object staging proof passed through the Rust AWS SDK resolver: `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin backfill_object_staging -- --spec /private/tmp/bte-binance-local-proof-20260607/backfill-object-staging-s3-spec.toml` wrote `s3://bolt-parquet/nt-research-analytics/raw/source=data.binance.vision/product=spot/frequency=daily/family=trades/symbol=BNBUSDC/dt=2026-03-01/object=433d32b8d828abee5e1937e01372d16f7edadc14c41fe736b0b9577541fa5e81.zip`; manifest content hash was `863cf743c611b4bd404a86c0ed72699ae13d8ec3e070355a8b15a065456a66be`, local manifest file SHA256 was `37b55b23a49396171b53d511a26b7f547b89dbcbeaf0d65e756807de33df0fef`, and S3 `head-object` confirmed `ContentLength = 1066394`.
- The real S3 source-proof evidence staging proof then passed through the same Rust SSM/object-store path: `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin source_proof_evidence_staging -- --spec /private/tmp/bte-binance-local-proof-20260607/source-proof-evidence-staging-s3-spec.toml` wrote three objects under `s3://bolt-parquet/nt-research-analytics/source-proofs/v1/source_binding=binance-spot-native-trades/proof=source-proof-binance-spot-native-trades-scratch/version=1/`, manifest content hash `18d496243bc002466f972f82eb24c30890b55fa2e337eff79c628804b76deffe`, local manifest file SHA256 `e3a2a2fafb0ba02c04d84942f54d7de7411f74b798232f500056f45026dbc7d8`, `record_count = 3`, and `total_bytes = 1005`.
- The accepted Binance candidate then replaced the scratch proof evidence under a new proof id, `source-proof-binance-spot-native-trades-bnbusdc-2026-03-01`. The SSM-backed evidence staging command `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin source_proof_evidence_staging -- --spec /private/tmp/bte-binance-accepted-proof-20260607/staging-specs/source-proof-evidence-staging-s3-spec.toml` wrote three accepted-candidate evidence objects totaling 5,938 bytes under `source-proofs/v1/source_binding=binance-spot-native-trades/proof=source-proof-binance-spot-native-trades-bnbusdc-2026-03-01/version=1/`: schema sample SHA256 `22ec3fcfde7a72347dd059a88da31d0fb8fe2c585216190e5c2df2ca01ea1265`, official Binance public-data README SHA256 `085ab91377aa9325d44f4c7ad27cce4ab381e158403e1d7df2bad39d1a66f7c6`, and current-only instrument summary SHA256 `e4e3b2e373d1334d5a7c384ae6a36b956cb2f84611867e70000d57ad71c51da6`; evidence-staging manifest hash was `60e6b2bc6f9ef3fa0dc2405edc8b92e28bc15f2057fa9c60916e35b58f28c5fc`.
- The accepted Binance source proof uses official Binance public-data README license evidence and the generic claim-limited instrument-universe fallback: `required_checks.instrument_universe.outcome = not_applicable`, with `evidence_ref` equal to the current-only instrument summary S3 URI, and a matching blocking claim limit forbidding historical venue-rule, fillability, rounding, sizing, or execution-quality claims. `source_proof_admissibility` over `/private/tmp/bte-binance-accepted-proof-20260607/source-proof.json` returned `accept_ready_records = 1`, `current_contract_rejected_records = 0`, `non_current_contract_records = 0`, content hash `394a89ca8dae92618c24802751c66d3af46f97602e5ab231f7b80f727c2278e1`.
- The exact accepted Binance source proof and run spec used for the S3 publication are committed as reference artifacts at `specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-01.json` and `specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.binance-bnbusdc-2026-03-01.toml`.
- The Binance backfill gates then selected exactly one object without downloading payloads: `backfill_source_proof_scope` wrote `/private/tmp/bte-binance-accepted-proof-20260607/scope/backfill-source-proof-scope-report.json`, content hash `08d95767d22098e91b3d69a7b09cbf0496c931f63f3bafecf5dbe7dfbe436a39`, status `CandidateFound`, `matching_object_count = 1`, `object_level_tranche_required = false`; `backfill_accepted_tranche` wrote `/private/tmp/bte-binance-accepted-proof-20260607/tranche/backfill-accepted-tranche-manifest.json`, content hash `040120fff063fe2f767ef93da082612c9ca917a72dd379bd7386a92e3237cd8c`, status `Accepted`, `object_count = 1`, `accepted_bytes = 1066394`; corrected `backfill_execution_plan` wrote `/private/tmp/bte-binance-accepted-proof-20260607/plan-corrected/backfill-execution-plan.json`, content hash `08927d6c0dc167186c37f42aadab02a1641437379f4fb4762e58b5af7fd8d25a`, status `Ready`, `object_count = 1`, `accepted_bytes = 1066394`, and `operator_run_id = backtesting-vertical-slice-binance-bnbusdc-2026-03-01-accepted`.
- The corrected accepted Binance operator proof then ran end-to-end through the same path, not a new converter: `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin backtesting-vertical-slice -- --run-spec /private/tmp/bte-binance-accepted-proof-20260607/run-corrected.toml --execution-plan /private/tmp/bte-binance-accepted-proof-20260607/plan-corrected/backfill-execution-plan.json --object /private/tmp/bte-binance-bnbusdc-2026-03-01.zip --output-dir /private/tmp/bte-binance-accepted-proof-20260607/output-corrected --publish-output --prove-published-catalog`. It produced `71431` canonical rows, `71431` NT catalog read-back ticks, NT version `6e059dcbb59ac1e582132fc431a581936c216c3c`, catalog hash `8c128fe5acbb2e0df7c0f9b30d80de16acb285ca95f67a7bfc08c969f6b48362`, `nt_result.iterations = 71431`, zero events/orders/positions, `published_catalog_direct_s3 = true`, and `published_catalog_iterations = 71431/71431`. Local SHA256 values were `backtest-result-contract.json = aafc75b721a1b532fbba0fa98ca53f1968aa5e61ad3e2079718e602f546326fb`, `accepted-source-proof.json = 05f2c1b77e169a902577a3b4aab0cb2a0a90ea024ab8c606ae1e3f6e641e0557`, `backtest-run-manifest.json = ed70daa033b0a62aeeadfa4a969661ca4c74cd20e664c201bad1c134e2fab853`, and `published-catalog-proof.json = 134d99c278e22a743bd4866afc46965bacf44093b86a7d5185ce7cec15fbfd98`.
- Direct S3 listing of `s3://bolt-parquet/nt-research-analytics/backtests/backtesting-vertical-slice-binance-bnbusdc-2026-03-01-accepted/` after the corrected accepted Binance proof returned 10 objects totaling 5,442,092 bytes: accepted-source-proof, backtest-run-manifest, canonical-trades parquet, conversion checkpoint/manifest, NT catalog instrument parquet, NT catalog trades parquet, published-catalog-proof, catalog-metadata, and backtest-result-contract. `published-catalog-proof.json` contains `direct_s3_catalog_access_proven = true`, `expected_iterations = 71431`, `nt_iterations = 71431`, and catalog URI `s3://bolt-parquet/nt-research-analytics/backtests/backtesting-vertical-slice-binance-bnbusdc-2026-03-01-accepted/nt-catalog`.
- The first Binance publication at `s3://bolt-parquet/nt-research-analytics/backtests/backtesting-vertical-slice-binance-bnbusdc-2026-03-01/` is superseded and non-promotable because its published accepted-source-proof carried stale scratch `required_checks.license` evidence. It remains forensic only; the corrected `-accepted` prefix is the accepted Binance S3 proof.
- Fresh recursive S3 listing after the corrected accepted Binance publication returned `37` objects and `12,067,580` total bytes under `s3://bolt-parquet/nt-research-analytics/`: two clean promotable backtest catalog/result trees, one superseded forensic Binance backtest tree, the accepted Binance daily raw ZIP, the accepted Binance source-proof evidence set, and the earlier scratch evidence set retained as forensic staging evidence.
- The main operator S3 publish/proven-catalog proof then ran successfully with SSM-enabled temp run-spec `/private/tmp/bte-binance-local-proof-20260607/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.s3-publish.toml`, regenerated execution plan `/private/tmp/bte-binance-local-proof-20260607/backfill-execution-plan-reference-trades-s3-publish-output/backfill-execution-plan.json` (`content_hash = 1f633500ff09967b5d87844ade41214c64cea0ecf4d66c6271ef940e3d272f4f`, `status = Ready`, `accepted_bytes = 8505`), and command `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin backtesting-vertical-slice -- --run-spec /private/tmp/bte-binance-local-proof-20260607/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.s3-publish.toml --execution-plan /private/tmp/bte-binance-local-proof-20260607/backfill-execution-plan-reference-trades-s3-publish-output/backfill-execution-plan.json --object /private/tmp/bte-bnbusdc-current-schema-object.csv.gz --output-dir /private/tmp/bte-binance-local-proof-20260607/out-s3-publish-main-operator --publish-output --prove-published-catalog`.
- That main S3 proof produced `937` canonical rows, `937` NT catalog read-back ticks, NT version `6e059dcbb59ac1e582132fc431a581936c216c3c`, catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`, `nt_result.iterations = 937`, zero events/orders/positions, `published_catalog_direct_s3 = true`, and `published_catalog_iterations = 937/937`. Local SHA256 values were `backtest-result-contract.json = 1cad7fb6c764a34af2384f0f1d92978bc5ecc07972a1320c0d58280ea344bed4`, `backtest-run-manifest.json = e590b27ba07541a3409c4e923072125bed2d0fb0ec20ab9292c4d3a8741ce086`, and `published-catalog-proof.json = d3721f6bc342eca63a6fe5cd68669d146847c5eb531de9da14398c58191a104b`.
- Direct S3 listing of `s3://bolt-parquet/nt-research-analytics/backtests/backtesting-vertical-slice-bnbusdc-2026-03-01/` after the main proof returned 10 objects totaling 110,448 bytes: accepted-source-proof, backtest-run-manifest, canonical-trades parquet, conversion checkpoint/manifest, NT catalog instrument parquet, NT catalog trades parquet, published-catalog-proof, catalog-metadata, and backtest-result-contract. `published-catalog-proof.json` contains `direct_s3_catalog_access_proven = true`, `expected_iterations = 937`, `nt_iterations = 937`, and catalog URI `s3://bolt-parquet/nt-research-analytics/backtests/backtesting-vertical-slice-bnbusdc-2026-03-01/nt-catalog`.
- Converted artifacts were intentionally deleted by the user to avoid confusing stale partial outputs with accepted clean outputs
- Local rebuilt-binary run against the accepted raw object wrote `/tmp/bte-real-e2e.RcFHhT/out` and produced `937` canonical rows, `937` NT catalog read-back trade ticks, NT `BacktestNode` iterations `937`, and catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`
- Earlier pre-raw-payload-schema local generic-converter CLI proof into `/private/tmp/bte-s3-proof/out-local-generic-converter` produced converter config hash `4e54ce1edbdab877a776cb5d38ede603a747da49c0355f80b2f3665905333080`, conversion manifest hash `7d6d48376c026174bb84830dc6058e4eddecf9e3632344431413a4b2b3ca8352`, conversion checkpoint hash `60429ebd758ec1b0383dbedd0d0e38997a0bc90f33f2dc2ba2bf5bf6b1bd5842`, catalog metadata hash `3b9ee2bd6980de74aa30b677a408f073d5f68ff6aaf81a338425a2924709e587`, catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`, row count `937`, and NT iterations `937`; after adding `[converter.raw_payload]` and byte bounds, these pre-schema hashes are forensic evidence only
- Fresh current-schema local CLI run against the accepted raw object wrote `/private/tmp/bte-bnbusdc-current-schema-out-20260607a` and produced `937` canonical rows, `937` NT catalog read-back trade ticks, NT `BacktestNode` iterations `937`, catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`, converter config hash `a20a83ef7bf42926e394f16c43c09819b58fc8c08d42ceb23352f9a61e293144`, conversion manifest hash `a35705f61ffc42c6fb019fa4e457e9d655d63825cd6401383c0977bafc951ccb`, conversion checkpoint hash `3c77e46b26b6998ce0edba532b3a608521490b2afb61e3a304e121a0b74ae0e5`, and local catalog metadata hash `e552c285d2fb52521a38add9fbe94af360a5eb88e34dd63e8edca25f19a0a0e9`; the committed portable reference catalog metadata hash is `f82bd70268d1df4163c1746ad79194fc987082e4b6ab9cdc82d6d8275990e882` because it uses reference URIs instead of the local `/private/tmp` execution URI
- Fresh release-binary local CLI run against the accepted raw object wrote `/tmp/bte-real-cli.sDvExP/out-local` and produced `937` canonical rows, `937` NT catalog read-back trade ticks, NT `BacktestNode` iterations `937`, and catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`
- Fresh release-binary `--publish-output` run against the accepted raw object and a local `file://` output prefix published 8 artifacts; `diff -qr /tmp/bte-real-cli.sDvExP/out-publish /tmp/bte-real-cli.sDvExP/publish-root/backtests/backtesting-vertical-slice-bnbusdc-2026-03-01` returned no differences
- Fresh current-branch local CLI run after SSM artifact-store changes wrote `/private/tmp/bte-s3-proof/out-local-after-ssm-artifact-store-3` and produced `937` canonical rows, `937` NT catalog read-back trade ticks, NT `BacktestNode` iterations `937`, and catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`
- Fresh current-branch S3 publish/proof command with only `region` configured failed before the backtest with `artifact_store.ssm_parameters must resolve access_key_id and secret_access_key before publishing to an s3 output_prefix`; `/private/tmp/bte-s3-proof/out-s3-missing-ssm-fail-fast` was not created
- Earlier non-secret SSM parameter-name searches for `artifact`, `backtest`, `s3`, `parquet`, `research`, `nt`, and `credential` returned no names; after the user pointed to the AWS API credentials, create-only SSM SecureString parameters were created under `/bolt/artifact-store/s3/` and proven by the raw/evidence S3 staging gates without printing secret values
- Accepted-tranche manifest CLI selected the first reference trades tranche from the source-proof scope report without payload download or venue-specific branching. It wrote `/private/tmp/bte-coverage-ledger-20260607/backfill-accepted-tranche-reference-trades-output/backfill-accepted-tranche-manifest.json`, content hash `90078dfb15f2056b122ced643a8072d088ea9d69b3b0e0afaf73d7b941b95c26`, status `accepted`, `object_count = 1`, `accepted_bytes = 8505`, and bound scope-report hash `db110e9baf7a6bf710cbb35387424b3867404f79a4afd022cf85917c8b910e3b`.
- Backfill execution-plan CLI then bound that accepted tranche to the committed operator run-spec before payload fetch. It wrote `/private/tmp/bte-coverage-ledger-20260607/backfill-execution-plan-reference-trades-output-v2/backfill-execution-plan.json`, content hash `ae4e65626fff25ef3fcd07d60fe4305802d3451814111c62e2f8e42e9e128b9f`, file SHA256 `3681f44ec40bb32f755c91986e7cb18fb814f274b12d94d30ba8d80f4a106ee8`, accepted-tranche manifest hash `996ee9c05aecfcb9a37ce4af82e4242e60f99db2ff98e0f15f0467bfce4fa90f`, run-spec hash `edaa6fcabc782775b610e10699b940c98acfd7c2ef9d659f627a86607392bb2a`, status `ready`, `object_count = 1`, `accepted_bytes = 8505`, `max_object_bytes = 8505`, `max_decoded_bytes = 1048576`, and zero blocking issues.
- Static sample-token scan across the new execution-plan source, CLI, and tests found no current sample provider/venue/instrument/proof tokens, so the gate is source-proof/run-spec driven rather than sample-driven.
- Main operator CLI execution-plan validation was added test-first: `cli_execution_plan_mismatch_rejects_before_reading_object` RED failed with `struct Cli has no field named execution_plan`; GREEN passed after the CLI read the execution-plan JSON, compared its run-spec hash and bound object fields against the submitted run-spec, and rejected mismatches before invoking the object reader. A later mandatory-plan RED, `cli_requires_execution_plan_before_reading_object`, failed because the object reader was still called when no plan was supplied; GREEN passed after `--execution-plan` became a required CLI argument and the plan byte-budget gate became the first object-size boundary. The focused CLI bin test target reported 7 passed, covering explicit publish opt-in, published-catalog proof flag gating, size-mismatch stat-before-read, required execution-plan parsing, plan byte-budget pre-read rejection, S3 publish preflight before object read, and execution-plan mismatch before object read.
- Run-manifest unsupported-surface validation is now also pre-object-read and
  pre-derived-artifact. `cli_rejects_unsupported_catalog_data_type_before_reading_object`
  RED failed because a run spec with a structurally valid but unsupported
  `catalog_input.data_type` still invoked the object reader. GREEN passed after
  the CLI began validating the local run manifest from the run spec's accepted
  object hash before object read, while `run_from_run_spec` still recomputes
  the object hash from bytes before conversion/backtest. The operator now
  shares the same accepted-dataset/local-manifest helper, and `run_backtest`
  performs manifest validation before canonical normalization writes. Focused
  verification covered 9 CLI tests, `run_from_run_spec_produces_artifacts`, and
  `run_from_run_spec_reuses_completed_output_without_rebuilding_catalog`.
- A real CLI smoke check with the ready execution plan and a deliberately missing object path failed at `stat object /private/tmp/bte-coverage-ledger-20260607/does-not-exist.csv.gz`, which proves the valid plan was accepted and the command stopped at the object stat boundary before conversion, catalog projection, or backtest work.
- After the mandatory `--execution-plan` change, the same cheap CLI smoke shape was rerun with the ready execution plan and a deliberately missing object path, and failed at `stat object /private/tmp/bte-coverage-ledger-20260607/does-not-exist-after-mandatory-plan.csv.gz`; this proves the required-plan CLI path still validates the ready plan and stops at the object stat boundary before conversion, catalog projection, or backtest work.
- A real plan-bound full CLI run then used the ready execution plan and accepted raw object to write `/private/tmp/bte-plan-bound-cli-e2e-20260607/output`. It produced `937` canonical rows, `937` NT catalog read-back `TradeTick` rows, NT `BacktestNode` iterations `937`, catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`, accepted object hash `d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598`, NT revision `6e059dcbb59ac1e582132fc431a581936c216c3c`, and fidelity class `TRADE_REPLAY`.
- The plan-bound full CLI rerun against the same output directory also exited 0 and kept deterministic artifacts stable: `canonical-trades.parquet` hash `007fd929557b08e314481fbf456736fb045a46e4a30761bbcab2f02dd687f1c4`, `backtest-run-manifest.json` hash `770618e30b0cc5fbc33388fd4ff692cab708bd5e6c69b01b6c2f3d7444c7cf3a`, `conversion-manifest.json` hash `886cd87aa3ec691780266d9a409f4218edd9114afaa63884b589102cbe7103a1`, `conversion-checkpoint.json` hash `d607c511e875b8110d381707228e3a66419411f912ae7e1f849d3656d0e61c4a`, and `catalog-metadata.json` hash `1e918bfc1799d974b9c0286f19e4899dc9096d726f782540008b918d9c9edfcf`. The result-contract file hash changed across runs because it carries run-instance fields, while the bound source proof, converter, manifest, conversion, catalog, NT revision, and NT result facts remained tied to the accepted object.
- Accepted raw object was then downloaded as the single bounded proof object to `/private/tmp/bte-accepted-tranche-reference-trades-20260607/raw/d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598.csv.gz`; `wc -c` returned `8505`, and SHA256 matched `d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598`.
- The rust-verification Cargo wrapper refused a fresh `cargo run` because free disk was below its 10 GB threshold, so verification reused the already-built operator binary rather than producing more Cargo build output.
- Already-built operator run against the accepted object wrote `/private/tmp/bte-accepted-tranche-reference-trades-20260607/output` and produced `937` canonical rows, NT catalog root `/private/tmp/bte-accepted-tranche-reference-trades-20260607/output/nt-catalog`, catalog hash `530167268245f7b7f484391653e5be172a1f921694c5f14c371beda687fa984f`, NT catalog read-back `937` `TradeTick` rows, NT revision `6e059dcbb59ac1e582132fc431a581936c216c3c`, NT `BacktestNode` iterations `937`, `total_events = 0`, `total_orders = 0`, `total_positions = 0`, and result-contract fidelity class `TRADE_REPLAY`.
- Accepted-tranche rerun with the same already-built operator reused the same accepted object/output directory and again processed `937` NT iterations. Deterministic artifacts stayed stable: `canonical-trades.parquet` hash `007fd929557b08e314481fbf456736fb045a46e4a30761bbcab2f02dd687f1c4`, `catalog-metadata.json` hash `e89119627f9256a62416bb06a009f0fd371fc33ba2919c1b4b7eb4c3d9900451`, `conversion-manifest.json` hash `886cd87aa3ec691780266d9a409f4218edd9114afaa63884b589102cbe7103a1`, `conversion-checkpoint.json` hash `d607c511e875b8110d381707228e3a66419411f912ae7e1f849d3656d0e61c4a`, and `backtest-run-manifest.json` hash `770618e30b0cc5fbc33388fd4ff692cab708bd5e6c69b01b6c2f3d7444c7cf3a`. The result-contract file hash changed because the contract carries run-instance fields, but its bound source proof, converter fingerprint, catalog hash, conversion hashes, NT revision, and NT result fields remained tied to the accepted object.
- The accepted-tranche output catalog contains NT-owned typed Parquet files at `nt-catalog/data/trades/BNBUSDC.BYBIT/2026-03-01T00-00-01-665000000Z_2026-03-01T23-50-46-022000000Z.parquet` and `nt-catalog/data/instruments/BNBUSDC.BYBIT/1970-01-01T00-00-00-000000000Z_1970-01-01T00-00-00-000000000Z.parquet`; Bolt did not write a custom catalog format.
- Verification strategy for the source-proof preflight blocker-ranking update intentionally avoided broad Cargo suites. The fresh checks were `cargo fmt`, `cargo fmt --check`, focused migration-preflight and legacy-derivability integration tests, focused clippy with warnings denied, real append-only source-proof migration preflight rerun, artifact hash inspection, and `git diff --check`.

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
| BacktestDataConfig / catalog cloud path | Yes, proven for bounded S3 catalog publications and a multi-instrument S3 fixture | NT `BacktestDataConfig` carries `catalog_path`, `catalog_fs_protocol`, generic storage options, Rust storage options, `instrument_id`, and `instrument_ids` (`crates/backtest/src/config.rs:595-725`). `BacktestNode` builds `protocol://catalog_path`, chooses Rust options over generic options, and calls `ParquetDataCatalog::from_uri` (`crates/backtest/src/node.rs:503-512`). `nautilus-persistence` `cloud` enables S3/Azure/GCP/HTTP object stores (`crates/persistence/src/lib.rs:36`). BTE has now run `--publish-output --prove-published-catalog` through SSM-backed artifact-store options for the Bybit sample/reference tranche (`937/937` NT iterations), the corrected Binance daily tranche (`71431/71431` NT iterations), and the `nt_catalog_proof` multi-instrument S3 fixture (`2` instruments, `6` ticks, `6` NT iterations). The manifest path now maps configured `instrument_ids` into NT `BacktestDataConfig::instrument_ids` for the bounded L2 data-config slice (`crates/backtesting-vertical-slice/src/run_manifest.rs:1448-1524`). | Use NT cloud catalog config for accepted bounded tranches; broad historical S3 coverage still requires the same proof artifact per accepted tranche. Use configured `instrument_ids` only where source-proof/claim semantics are explicit; other query knobs still fail closed. |
| NT catalog data classes | NT supports more than current BTE exposes | NT dispatches `QuoteTick`, `TradeTick`, `Bar`, `OrderBookDelta`, `OrderBookDepth10`, `MarkPriceUpdate`, `IndexPriceUpdate`, `InstrumentStatus`, and `InstrumentClose` from `BacktestDataConfig` (`crates/backtest/src/node.rs:526-568`). Current BTE manifest maps `"TradeTick"` and `"OrderBookDelta"` only (`crates/backtesting-vertical-slice/src/run_manifest.rs:1448-1458`, `1760-1764`, `1985-1993`). `catalog_projection.rs` now hashes and read-back checks NT-native `BinaryOption`, `OrderBookDelta`, and `TradeTick` fixture catalogs. | Keep adding NT data classes through typed manifest/projection support, not stringly venue branches. `OrderBookDelta` manifest mapping and NT-native BinaryOption L2 catalog hash/read-back are proven; PMXT source-backed projection is still open. |
| BacktestVenueConfig / venue simulation controls | Yes | NT `BacktestVenueConfig` exposes primitive controls and complex model surfaces: routing, frozen account, order flags, bar/trade execution, liquidity, queue, OTO mode, base currency, default/per-instrument leverage, margin model, modules, fill/latency/fee models, price protection, and settlement prices (`crates/backtest/src/config.rs:331-389`). Current BTE maps primitive controls and declares unsupported complex model fields, rejecting them with `UnsupportedNtSurface` before NT config construction. | Use NT venue config. Keep complex models `unsupported_for_now` until each has a real typed, claim-limited NT mapping. |
| resume/checkpoint/idempotency | Partially: NT skips existing exact Parquet file and enforces disjoint intervals; NT does not know Bolt source-proof/converter checkpoint semantics | NT `write_to_parquet` returns without writing if the object already exists and checks interval disjointness before writing (`crates/persistence/src/backend/catalog.rs:537-556`). It does not bind source proof id/version, raw object hash, converter identity/version/config hash, conversion manifest hash, or checkpoint state. | Bolt owns a thin conversion boundary around NT output; do not rely on NT file-skip behavior as acceptance/idempotency proof. |
| artifact/proof governance | No | NT has no concept of Bolt `SourceProofReport`, accepted-object byte/hash gate, artifact-root typed subpaths, proof-pin policy, or objective `BacktestResultContract`. Current BTE owns those in `source_proof.rs`, `conversion_boundary.rs`, `run_manifest.rs`, and `result_contract.rs`. | Bolt-owned governance is necessary and must stay outside simulation/execution truth. |

Detailed extension-surface classification is recorded separately in `backtest-extension-surface-matrix.md`.

| Surface | NT capability | Bolt current use | Status |
| --- | --- | --- | --- |
| Instrument model | `CurrencyPair::new_checked`, `Price`, `Quantity`, `Money`, `InstrumentId`, `Symbol` | `catalog_projection.rs` builds the NT instrument from accepted instrument metadata, using checked constructors and NT precision | Uses NT |
| Market data model | `TradeTick` | canonical trade rows are converted into NT `TradeTick` values | Uses NT |
| Raw source -> canonical trades | NT does not parse this project's accepted raw archive/proof contract into project canonical rows | Bolt owns a generic `csv-native-trades-to-canonical-trades.v1` adapter driven by `[converter.raw_payload]` object-container config plus `[converter.csv]` TOML column, timestamp-unit, and side-token mapping; source/venue values remain in source proof and run-spec data | Correct Bolt-owned adapter; no operator/runner venue hardcode |
| Catalog writer | `ParquetDataCatalog::write_instruments`, `write_to_parquet` | catalog projection writes trade-path instruments/ticks through NT; bounded fixture coverage now writes `InstrumentAny::BinaryOption`, `OrderBookDelta`, and `TradeTick` through NT and reads them back before hashing | Uses NT |
| Catalog reader | `ParquetDataCatalog::query_typed_data` / typed query path | read-back proof loads NT `TradeTick`s and compares against accepted rows | Uses NT |
| Backtest execution | `BacktestNode` + `BacktestRunConfig` | runner builds an NT run config, builds the node, injects the manifest strategy, and runs NT | Uses NT |
| Data loading into engine | `BacktestNode` creates `ParquetDataCatalog` from `BacktestDataConfig` and dispatches by `NautilusDataType` | manifest maps catalog input to `TradeTick` and `OrderBookDelta`; engine consumption is checked by NT iteration count for the trade path, while the bounded PMXT L2 catalog/BacktestNode proof is still pending | Uses NT |
| Catalog cloud configuration | `BacktestDataConfig` supports `catalog_fs_protocol`, `catalog_fs_storage_options`, and `catalog_fs_rust_storage_options`; `BacktestNode` passes them to `ParquetDataCatalog::from_uri` | manifest declares these fields and maps them into NT. Local operator mode resets them to `NONE` while binding a local projection root. Published proof mode rebuilds a manifest for the published catalog URI and runs NT `BacktestNode` against that S3 catalog with resolved SSM-backed object-store options. The separate `nt_catalog_proof` command proves the same NT catalog path with `instrument_ids` over a two-instrument S3 fixture | Uses NT config surface; direct S3 catalog execution is proven for two bounded accepted/reference tranches and one bounded multi-instrument fixture |
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
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked market_structure_fixture -- --nocapture`: RED first failed with missing `SourceBindingMetadata.market_structure_fixture` and `FixtureType::BinaryOption`; after adding the registry field, RED failed again because a registry/proof pair using legacy `prediction-market` could still be accepted
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --bin backtesting-vertical-slice cli_uses_run_spec_source_bindings_path_before_reading_object -- --nocapture`: RED failed with `source_binding "runtime-synthetic-native-trades" for venue "runtime-synthetic" is not configured in the registry`, proving operator acceptance still used the embedded committed registry instead of the run-spec `source_bindings_path`
- `just bte-test rejects_non_latest_proof_pin_without_reason_code` failed because `BacktestingRunManifest::validate()` accepted an audit run that set `pins_non_latest_proof = true` without a structured reason code
- `just bte-test rejects_audit_non_latest_proof_pin_without_reason_detail` failed because `BacktestingRunManifest::validate()` accepted an `audit_or_investigation` proof pin without detail
- `just bte-test accepts_all_configured_non_latest_proof_pin_reason_codes_from_toml` failed because TOML accepted only `baseline_reproduction`, `audit_or_investigation`, and `migration_validation`, while the plan/reference contract also allows `published_result_reproduction` and `regression_comparison`
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked artifact_manifest_records_deferred_currentness_rule_slots -- --nocapture`: RED failed with missing `BacktestRunManifestArtifact.currentness_rule_slots`, `ManifestCurrentnessRuleSlot`, `ManifestCurrentnessDimension`, and `ManifestCurrentnessRuleStatus`, proving the manifest artifact had no machine-readable slots for deferred NT version, strategy config hash, catalog hash, manifest schema, or execution-model currentness rules

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
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked run_manifest::tests -- --nocapture`: 63 focused manifest tests passed, covering `BacktestingRunManifest` validation, TOML round-trip, run id/window mapping into `BacktestRunConfig`, primitive venue controls into `BacktestVenueConfig`, catalog path/protocol/storage/instrument mapping into `BacktestDataConfig`, resolved NT surface recording, and structured rejection of unsupported NT engine, venue-model, and catalog-query surfaces before NT config construction
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
- `just bte-test run_manifest::tests::l2_replay_accepts_order_book_delta_data_config`: RED failed with `UnsupportedDataType { data_type: "OrderBookDelta" }`; GREEN passed after the manifest mapped `OrderBookDelta` into NT `NautilusDataType::OrderBookDelta` and bound `OrderBookDelta` to `L2Replay`
- `just bte-test run_manifest::tests::data_config_maps_configured_multi_instrument_ids`: RED failed with `UnsupportedNtSurface { field: "catalog_input.instrument_ids" }`; GREEN passed after the manifest mapped configured `instrument_ids` into NT `BacktestDataConfig::instrument_ids`
- `just bte-test run_manifest::tests::`: 65 focused manifest tests passed after the bounded L2 manifest/data-config slice
- `just bte-fmt-check`: passed after rustfmt normalized the BTE end-to-end test import list
- `just bte-test catalog_projection::tests::binary_option_l2_catalog_records_round_trip_through_nt_catalog`: RED failed after NT wrote/read the fixture catalog because `logical_catalog_hash` rejected `InstrumentAny::BinaryOption`; GREEN passed after the logical hash covered `BinaryOption` instruments and `OrderBookDelta` rows
- `just bte-test catalog_projection::tests::`: 16 focused catalog projection tests passed after adding bounded NT-native `BinaryOption`/`OrderBookDelta`/`TradeTick` catalog hash/read-back coverage
- `just bte-fmt-check`: passed after rustfmt normalized the new catalog projection test
- `just bte-test first_proof_selector_uses_configured_event_roles_without_asset_constants`: RED failed because the new first-proof selector stub returned `Blocked` instead of `Selected`; GREEN passed after the selector applied configured required/excluded event families, row budget, and deterministic `replay_rows`/`asset_id` ordering without production venue or asset constants
- `just bte-test first_proof_selector_writer_is_config_and_ledger_driven_and_idempotent`: RED failed with the explicit unimplemented writer error; GREEN passed after TOML spec parsing, JSON event-count ledger parsing, idempotent report writing, `event_count_ledger_hash`, and `selected_asset_ids_hash` support
- `just bte-test first_proof_selector`: 2 focused selector tests passed
- `just bte-fmt-check`: passed after first-proof selector changes
- `just bte-test result_contract::tests::l2_result_contract_requires_event_count_ledger_hash`: RED failed because an `L2Replay` contract without selector provenance still validated; GREEN passed after `L2Replay` contracts required `event_count_ledger_hash`
- `just bte-test result_contract::tests::l2_result_contract_requires_and_binds_selected_asset_ids_hash`: RED failed because an `L2Replay` contract with only `event_count_ledger_hash` still validated; GREEN passed after `L2Replay` contracts required `selected_asset_ids_hash` and serialized both selector hashes
- `just bte-test runner::tests::l2_run_contract_provenance_requires_selector_hashes`: RED failed with unresolved `BacktestSelectorProvenance` and `selector_provenance_hashes`; GREEN passed after runner inputs gained generic selector provenance and L2 replay run-contract construction refused missing selector hashes
- `just bte-test runner::tests:: backtesting_vertical_slice_end_to_end result_contract::tests::`: 35 focused tests passed after runner selector-provenance wiring
- `rust_verification cargo test --test backtesting_vertical_slice_catalog_and_node catalog_round_trips_binary_option_l2_deltas_and_node_consumes_them`: RED failed with the explicit unimplemented L2 BacktestNode proof helper; first GREEN attempt exposed NT matching-engine precision enforcement for L2 delta size precision; GREEN passed after the BinaryOption fixture wrote two OrderBookDelta rows to ParquetDataCatalog, queried them back, and ran BacktestNode with `BookType::L2_MBP` for exactly two iterations
- `rust_verification cargo test --test backtesting_vertical_slice_catalog_and_node`: 2 focused catalog/node tests passed after the NT-native L2 proof
- `rust_verification cargo fmt --check`: passed after the NT-native L2 proof
- managed BTE clippy: passed after the NT-native L2 proof
- `just bte-test artifact_root_resolves_typed_subpaths_without_extra_root_knobs rejects_unsupported_artifact_root_scheme`: 2 passed after RED compile failure on missing typed artifact-subpath API
- `just bte-test run_from_run_spec_rejects_object_byte_count_mismatch_before_artifacts`: RED failed because the operator ran NT instead of rejecting the byte mismatch, then GREEN passed after adding the pre-hash/pre-decompression byte-count check
- `just bte-test read_object_rejects_size_mismatch_before_loading_object`: RED failed with missing checked-read helper, then GREEN passed after adding CLI object-size preflight before `fs::read`
- `just bte-test backtest_index_record_generates_paths_under_single_artifact_root artifact_index_rejects_missing_lineage_and_non_sha256_hashes artifact_index_rejects_consumer_mutation_of_producer_records`: RED failed with missing `artifact_index` module, then GREEN passed after adding the pure Artifact Index contract helper
- `just bte-test lifecycle_config_rejects_default_delete_or_expiration_rules lifecycle_state_follows_configured_quiet_window lifecycle_config_requires_all_storage_profiles`: RED failed with missing lifecycle config types, then GREEN passed after adding the pure lifecycle policy helper
- `just bte-test committed_snapshot_resolution_rejects_hash_invalid_latest_pointer committed_snapshot_resolution_rejects_stale_latest_pointer committed_snapshot_resolution_requires_hot_index_metadata_active committed_snapshot_rejects_staged_or_orphan_records_as_discovery_truth latest_pointer_update_plan_uses_create_or_etag_preconditions`: RED failed with missing committed-snapshot/latest-pointer contract types, then GREEN passed after adding the pure Artifact Index commit-path helpers
- `just bte-test cross_kind_parent_resolution_uses_manifest_lineage_hashes cross_kind_parent_resolution_rejects_independent_latest_parent_hash`: RED failed with missing lineage-parent resolver, then GREEN passed after adding manifest-lineage parent resolution by artifact id and `sha256` hash
- `just bte-test immutable_event_create_is_idempotent_for_same_payload immutable_event_create_rejects_different_payload_at_same_uri`: RED failed with missing event object/create-plan contract types, then GREEN passed after adding the pure immutable event-create helper
- `just bte-test data_config_preserves_configured_object_store_conditional_put artifact_store_preserves_conditional_put_after_ssm_resolution artifact_store_rejects_disabled_conditional_put_for_s3_commit_path`: RED failed because the manifest rejected `conditional_put`; GREEN passed after preserving canonical `conditional_put = "etag"` through NT/object-store options and rejecting `disabled` for S3 Artifact Index commit readiness
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_artifact_index_commit_proof -- --nocapture`: RED failed with missing `artifact_index_commit_proof` module, then GREEN passed after adding the bounded commit proof runner and exercising create-only event/snapshot/audit writes, first pointer creation, ETag pointer update, stale ETag rejection, and pointer/snapshot readback resolution against an in-memory object store
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_artifact_index -- --nocapture`: 17 passed after adding the computed snapshot content-hash helper to the core Artifact Index contract
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin artifact_index_commit_proof -- --spec /private/tmp/bte-artifact-index-proof-20260608-3d6b1529/artifact-index-commit-proof-s3.toml`: real S3 proof passed through SSM-resolved artifact-store credentials without printing secret values; committed report file SHA256 `091501378ca70ca99b353d2252f040ef6b4d20c3d9b9e6db1dc29cc5d0489bf8`; event count `2`; snapshot count `2`; audit epoch count `2`; `latest_pointer_update_if_match_proven = true`; `stale_etag_update_rejected = true`; `direct_s3_commit_proven = true`; `producer_iam_scope_proven = false`
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_artifact_index_commit_proof -- --nocapture`: RED failed with missing `denied_artifact_kinds` and IAM-scope report fields, then GREEN passed after the proof runner began recording denied-kind probe attempts, permission rejections, and violation counts
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin artifact_index_commit_proof -- --spec /private/tmp/bte-artifact-index-iam-proof-20260608-ca7445ca/artifact-index-commit-proof-s3.toml`: real S3 IAM-scope probe passed as a negative result: the current SSM credential successfully wrote all three configured denied `research_analytics` Artifact Index paths under an isolated proof root, so `producer_iam_scope_proven = false`, `producer_iam_scope_denied_write_attempts = 3`, `producer_iam_scope_denied_write_rejections = 0`, and `producer_iam_scope_violation_count = 3`
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_artifact_index_iam_policy -- --nocapture`: RED failed with missing `artifact_index_iam_policy` module, then GREEN passed after adding a config-driven policy generator whose resources are scoped to the configured artifact kind and do not contain unrelated kind paths or `kind=*`
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_nt_dependency_proof -- --nocapture`: RED failed with missing `nt_dependency_proof` module, then GREEN passed after adding a Cargo.toml/Cargo.lock proof that binds the pinned NT git revision and required BTE features.
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_artifact_index -- --nocapture`: 17 passed, covering retain-forever lifecycle validation, default delete/expiration rejection, required `active`/`archive`/`deep_archive` storage profiles, configured quiet-window active-to-inactive transition, and active/queryable latest-pointer/current-snapshot metadata for committed discovery.
- `just bte-test acceptance_blocked_when_evidence_state_is_not_backfillable`: GREEN passed after source-proof acceptance began rejecting non-backfillable evidence states
- `just bte-test acceptance_blocked_when_source_binding_family_disagrees_with_registry`: GREEN passed after source-proof acceptance began binding proof `product_family`, `table_family`, and `evidence_state` to the TOML source-binding registry
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked market_structure_fixture -- --nocapture`: 6 focused tests passed after source-proof acceptance began requiring registry-owned `market_structure_fixture`, rejecting fixture mismatches, rejecting legacy non-BTE fixtures for accepted proofs, and proving committed TOML rows expose one `perps-spot` sample binding plus one `binary-option` candidate binding
- `just bte-test acceptance_blocked_when_source_binding_family_disagrees_with_registry acceptance_blocked_when_source_binding_missing_from_registry select_rejects_unknown_source_binding select_rejects_object_from_other_venue select_accepts_configured_source_host_with_url_variations`: 5 passed after source-proof acceptance began rejecting registry-missing source bindings and selection kept a forged-record guard
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --bin backtesting-vertical-slice cli_uses_run_spec_source_bindings_path_before_reading_object -- --nocapture`: 1 passed after operator acceptance and accepted-dataset selection loaded the run-spec `source_bindings_path`; the runtime-only synthetic source binding reached the object-reader sentinel instead of failing against the committed registry
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --test backtesting_vertical_slice_source_proof_admissibility_cli --test backtesting_vertical_slice_backfill_source_proof_scope -- --nocapture`: 4 passed after source-proof admissibility and source-proof scope report specs loaded the same configured source-bindings registry path
- `just bte-test select_rejects_same_host_path_outside_declared_source_template`: RED failed because a same-host Binance monthly trades object could satisfy the daily native-trades source binding; GREEN passed after selection began matching the object `source_url` path/query against the registered URI template
- Fresh focused recheck after the Binance BD permission-path clarification: `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked select_rejects_same_host_path_outside_declared_source_template -- --nocapture` passed `1` source-proof unit test, and `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_execution_plan -- --nocapture` passed `4` execution-plan tests. This verifies the no-repeat gates that reject same-host monthly/daily source mismatch and bind one accepted tranche object to one run-spec before payload fetch.
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked committed_binance_reference_binds_accepted_source_proof_without_scratch_evidence -- --nocapture`: RED failed with missing committed Binance reference constants; GREEN passed after adding the committed Binance run-spec/proof regression guard and normalizing the run-spec candidate proof evidence so it stamps into the committed accepted proof with no scratch evidence markers.
- `just bte-test rejects_non_latest_proof_pin_for_normal_run rejects_non_latest_proof_pin_without_reason_code rejects_audit_non_latest_proof_pin_without_reason_detail accepts_non_latest_reproduction_pin_with_reason_code`: 4 passed after adding typed non-latest proof-pin reason fields
- `just bte-test accepts_all_configured_non_latest_proof_pin_reason_codes_from_toml`: 1 passed after adding the missing `published_result_reproduction` and `regression_comparison` enum variants
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked artifact_manifest_records_deferred_currentness_rule_slots -- --nocapture`: 1 focused manifest unit test passed after `backtest-run-manifest.json` began recording the five generic deferred currentness-rule slots without venue/data-family-specific constants or new currentness enforcement semantics
- `just bte-test publish_output_artifacts_rejects_existing_published_artifact_without_overwrite`: RED failed because publish used default object-store overwrite semantics, then GREEN passed after bounded target preflight plus `PutMode::Create`
- `just bte-test publish_output_artifacts_rejects_existing_published_artifact_without_overwrite run_from_run_spec_and_publish_can_prove_published_catalog_consumption`: 2 passed after proof-mode publish stopped publishing the metadata/contract artifacts before the proof updates them
- `just bte-test cli_publish_preflight_rejects_missing_s3_ssm_before_reading_object`: RED failed because the CLI had no injectable pre-object-read publish preflight seam, then GREEN passed after publish storage options were resolved before local object read
- `just bte-test typed_unsupported_nt_venue_model_surfaces_parse_then_fail_before_nt_config`: RED failed with `E0599` because `ManifestError::UnsupportedNtSurface` and typed unsupported-surface schema did not exist
- `just bte-test rejects_unsupported_nt_venue_model_surface_requests_before_nt_config typed_unsupported_nt_venue_model_surfaces_parse_then_fail_before_nt_config`: 2 passed after adding optional manifest placeholders for NT leverage maps, margin model, modules, fill model, latency model, fee model, and settlement prices plus structured pre-NT rejection
- Earlier `just bte-test typed_unsupported_nt_catalog_query_surfaces_parse_then_fail_before_nt_config`: RED failed because `instrument_ids` under `[catalog_input]` was an unknown TOML field, proving pinned NT `BacktestDataConfig` query surfaces were not represented even as explicitly unsupported surfaces at that point; the later bounded L2 manifest slice supersedes only the `instrument_ids` part by mapping configured IDs into NT
- `just bte-test native_trade_source_bindings_cover_multiple_configured_venues`: RED failed with only `venues={"bybit"}` and `keys=["bybit-spot-tick-trades"]`; GREEN passed after adding `binance-spot-native-trades` as a registry-only candidate and requiring the test to exercise proof acceptance plus host selection for each configured native-trades binding
- `just bte-test acceptance_blocked_when_structured_scope_summary_missing`: RED failed with `E0609` because `SourceProofReport` had no `acceptance_scope` field; GREEN passed after adding structured scope facts and requiring them before proof acceptance
- `just bte-test acceptance_blocked_when_structured_scope_summary_has_failures_or_scope_violations`: RED failed because `evaluate_acceptance()` returned `Ok(())` with `failed_objects = 1`; GREEN passed after rejecting failed objects and selector-scope violations in the structured scope summary
- `just bte-test ledger_rejects_object_bytes_exceeding_structured_acceptance_scope`: RED failed because `select_accepted_dataset()` admitted an object whose bytes exceeded `acceptance_scope.accepted_bytes`; GREEN passed after object selection compares selected object bytes against the structured accepted byte count
- `just bte-test non_l2_fidelity_requires_structured_claim_limits structured_claim_limits_must_cover_forbidden_claims`: RED failed with `E0609` because `SourceProofReport` had no `claim_limits` field; GREEN passed after adding structured source-proof claim-limit rows and requiring every non-L2 `forbidden_claims` entry to be covered by a machine-readable limit
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked automated_acceptance_rejects_expired_required_check -- --nocapture`: RED failed because `RequiredCheck` had no expiry field and `AcceptanceError` had no expired-check variant; GREEN passed after optional `expires_at_utc` evidence expiry was added and source-proof acceptance rejects checks expiring before `coverage_time_range.end_utc`. `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked automated_acceptance_rejects_failed_required_checks -- --nocapture` passed, proving automated acceptance rejects every required-check slot when the outcome is `failed`.
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked staging_rejects_byte_mismatch_from_metadata_before_reading_payload -- --nocapture`: RED failed because object staging attempted to read an unreadable local payload before returning the configured byte mismatch; GREEN passed after staging checked filesystem metadata against `expected_bytes` before reading, hashing, or uploading the payload.
- `just bte-test accepted_data_flows_through_to_objective_result_contract`: RED failed because generated result contracts dropped structured source-proof claim-limit rows after acceptance and retained only plain canonical-table `forbidden_claims`; GREEN passed after `AcceptedDataset` began carrying `claim_limits` and runner/operator result-contract assembly consumed those structured rows
- `just bte-test --test backtesting_vertical_slice_end_to_end accepted_data_flows_through_to_objective_result_contract`: RED failed because the result contract claim limits carried only source-fidelity limits and no NT surface/default records; GREEN passed after generated result contracts appended resolved NT default, pass-through, and unsupported-surface claim-limit entries derived from `BacktestRunConfig`
- `just bte-test committed_result_contract_records_nt_extension_surface_claim_limits`: RED failed because the checked-in reference result contract lacked NT extension-surface claim limits; GREEN passed after updating the reference fixture
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked rejects_missing_ -- --nocapture`: RED failed because `BacktestResultContract::validate()` accepted blank final artifact URIs and blank NT pointer identity fields; GREEN passed after validation required every `artifact_uris.*` pointer, `nt_result.trader_id`, `nt_result.machine_id`, `nt_result.instance_id`, and rejected present-but-blank `nt_result.run_config_id`
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
- `just bte-test coverage_ledger_records_unsupported_manifest_schema_instead_of_aborting_batch`: RED failed with `E0599` because `BackfillCoverageIssue::UnsupportedManifestSchema` did not exist; GREEN passed after batch manifest ingestion began recording unsupported manifest schemas as rejected coverage records instead of aborting the whole ledger
- `just bte-test --test backtesting_vertical_slice_backfill_coverage`: 20 passed after unsupported manifest schemas became rejected coverage records
- `cargo test --test backtesting_vertical_slice_backfill_binding_coverage binding_coverage_blocks`: RED failed with missing `BackfillBindingCoverageIssue::UnconfiguredSourceBindingRecords` and `BackfillBindingCoverageIssue::EmptySourceBindingRecords`; GREEN passed after source-binding coverage blocks any ledger records with empty or unconfigured source bindings, even when another record exists for the required table family.
- `cargo test --test backtesting_vertical_slice_backfill_readiness readiness_blocks_when_binding_coverage_blocks`: RED failed because `evaluate_backfill_readiness` did not accept a binding-coverage report and `BackfillReadinessBlocker::BackfillBindingCoverageBlocked` did not exist; GREEN passed after combined readiness required the binding-coverage report and blocked whenever that report is not ready.
- `cargo test --test backtesting_vertical_slice_backfill_readiness readiness_blocks_when`: RED failed with missing `SelectedSourceBindingMismatch` and `SelectedSourceBindingMissingFromCoverage`; GREEN passed after combined readiness required the selected backfill record and selected source-proof candidate to share the same source binding and required binding coverage to include that selected binding with required table-family ledger records.
- `cargo test --test backtesting_vertical_slice_backfill_readiness selected_source_proof`: RED failed with missing `SelectedSourceProofMismatch`; GREEN passed after combined readiness required the selected backfill record and selected source-proof candidate to share the same source-proof id and version.
- `cargo test --test backtesting_vertical_slice_backfill_readiness selected_binding_has_no`: RED failed because readiness could report `ready` when selected binding coverage had zero accepted or zero canonical-ready records; GREEN passed after selected binding coverage required positive ledger, accepted, and canonical-ready counts for the required table family.
- `cargo test --test backtesting_vertical_slice_backfill_readiness readiness_blocks_when_binding_coverage_is_for_a_different_table_family`: RED failed because readiness could report `ready` when a `trades` readiness spec was paired with a ready `instruments` binding-coverage report for the same selected binding; GREEN passed after selected binding coverage required the binding's own `table_families` to include the readiness-required table family, instead of trusting a match flag computed for another report.
- `cargo test --test backtesting_vertical_slice_backfill_readiness readiness_blocks_when_selected_source_proof_candidate_has_acceptance_blockers`: RED failed because readiness could report `ready` when the selected source-proof migration candidate still carried `remaining_acceptance_blockers` such as a missing license check; GREEN passed after combined readiness treated any remaining source-proof acceptance blocker as a source-proof preflight blocker.
- `cargo test --test backtesting_vertical_slice_backfill_readiness readiness_blocks_when_backfill_preflight_did_not_require_canonical_ready`: RED failed because readiness could report `ready` when the joined backfill-preflight artifact was generated without `require_canonical_ready = true`; GREEN passed after combined readiness required that preflight selection flag before considering the canonical tranche side ready.
- `cargo test --test backtesting_vertical_slice_backfill_readiness readiness_blocks_when_selected_backfill_record_is_not_canonical_ready`: RED failed because readiness could report `ready` when the joined backfill-preflight artifact had an inconsistent selected record with `canonical_ready = false`; GREEN passed after combined readiness directly checked the selected record's canonical-ready flag.
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --test backtesting_vertical_slice_backfill_preflight backfill_preflight_selects_bounded_canonical_ready_record_without_source_constants -- --nocapture`: RED failed with `no field table_family on type BackfillPreflightSelectedRecord`, proving the selected backfill record did not preserve the ledger table family at the preflight handoff.
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --test backtesting_vertical_slice_backfill_readiness readiness_requires_selected_backfill_record_table_family_to_match_requested_path -- --nocapture`: RED failed with missing `SelectedBackfillTableFamilyMismatch`, proving readiness had no explicit blocker for a selected canonical-ready backfill record whose own table family differed from the requested path.
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --test backtesting_vertical_slice_backfill_preflight --test backtesting_vertical_slice_backfill_readiness -- --nocapture`: 23 passed after preflight selected records began carrying `table_family` and combined readiness began rejecting selected-backfill table-family mismatches.
- `cargo test --test backtesting_vertical_slice_backfill_readiness readiness_reads_toml_spec_and_writes_report_idempotently`: RED failed because readiness TOML rejected `[[supported_data_paths]]`; GREEN passed after the readiness spec and report began carrying TOML-owned supported table-family/NT-data-type pairs.
- `cargo test --test backtesting_vertical_slice_backfill_readiness readiness_blocks_when_required_nt_data_type_is_not_supported`: RED failed because readiness could report `ready` for `required_nt_data_type = "QuoteTick"` even though the configured supported path list only contained the proven `TradeTick` catalog projection path; GREEN passed after combined readiness rejected required NT data types absent from `supported_data_paths`.
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked kimchi_premium_source_proof -- --nocapture`: RED failed because `SourceProofReport` had no cross-market component proof fields; GREEN passed after adding generic point-in-time `cross_market_components`, requiring `korean_spot`, `reference_price`, `fx_quote`, and `token_mapping` roles for `product_category = "kimchi-premium"`, and rejecting component `event_time_utc` or `available_at_utc` values that would future-leak after `join_time_utc`. `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_sample_venue_guard -- --nocapture` passed after the production-code hardcode guard was extended to Korean spot venue-name examples.
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --test backtesting_vertical_slice_source_proof_legacy_derivability legacy_derivability_reports_structural_fields_without_accepting_source_proof -- --nocapture`: RED failed with missing `table_family_counts` and `blocking_issue_counts` fields on `SourceProofLegacyDerivabilitySummary`, proving the report forced manual record-level scans to explain why legacy proofs were blocked.
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --test backtesting_vertical_slice_source_proof_legacy_derivability -- --nocapture`: 3 passed after the legacy derivability summary began carrying deterministic aggregate table-family and blocker counts without accepting or mutating source proofs.
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --test backtesting_vertical_slice_source_proof_migration_preflight -- --nocapture`: 4 passed after the migration-preflight test fixture was updated for the expanded derivability summary contract.
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --test backtesting_vertical_slice_source_proof_migration_preflight migration_preflight_reports_source_binding_product_family_mismatch -- --nocapture`: RED failed with missing registry-aware preflight API, missing legacy metadata fields, and missing source-binding product-family mismatch blocker; GREEN passed after derivability preserved legacy `venue`/`product_family`/`evidence_state` fields and migration preflight loaded spec-owned source bindings before reporting remaining candidate acceptance blockers.
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --test backtesting_vertical_slice_source_proof_migration_preflight migration_preflight_prefers_candidate_with_fewer_remaining_blockers -- --nocapture`: RED failed because migration preflight selected the smaller synthetic candidate even though registry metadata made it less acceptable; GREEN passed after eligible candidates were ranked by remaining acceptance blocker count before payload size, record count, and proof URI.
- `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --test backtesting_vertical_slice_source_proof_migration_preflight migration_preflight_treats_non_backfillable_evidence_state_as_blocker -- --nocapture`: RED failed because migration preflight selected a smaller `bounded_or_current_only` synthetic candidate; GREEN passed after preflight mirrored source-proof acceptance by treating only `directly_backfillable` and `owner_archive_backfillable` evidence as backfillable.
- `cargo test --test backtesting_vertical_slice_backfill_readiness readiness_blocks_when_table_family_does_not_match_required_nt_data_type`: RED failed because readiness could report `ready` for `required_table_family = "quotes"` with `required_nt_data_type = "TradeTick"`; GREEN passed after combined readiness required the requested table-family/data-type pair to be present in TOML-owned `supported_data_paths`.
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
- `just bte-test`: 271 passed after recording unsupported manifest schemas as rejected coverage records, including 20 provider-agnostic backfill coverage tests, 1 provider-agnostic coverage CLI integration test, and 2 slow public API compile-fail tests
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
| Main reconciliation handoff | The original investigation branch was divergent from current `main`; successor branch `codex/bte-clean-converter-nt-use-main-reconcile` is now reconciled with `main` at `c1b1f7b49414008a11af11da24ebc49762debf54` and has passed the focused BTE verifier gates, but any PR/completion claim must use that successor branch, not the stale pre-reconciliation branch | use `codex/bte-clean-converter-nt-use-main-reconcile` as the authoritative BTE branch; before review, rerun the same focused BTE checks on the exact PR head and any required source-fence/repo guard |
| Broader production output coverage under `nt-research-analytics/` | Two clean reference backtest catalog/result paths are now published and direct-S3-proven, but broad historical backfill output is not | use the same SSM-backed publish path for each newly accepted tranche; verify S3 listing, artifact hashes, and direct catalog readback per tranche before treating outputs as research/analytics inputs |
| Artifact-store SSM parameter durability | The paths now exist and have been proven by real S3 raw/evidence staging writes and two main catalog/result publish proofs, but they were created during this investigation and should be treated as operational credentials | keep only the SSM parameter names in TOML (`/bolt/artifact-store/s3/access-key-id` and `/bolt/artifact-store/s3/secret-access-key` in `us-east-1`); never put secret values in TOML or logs; confirm rotation/ownership policy before long-running production backfill |
| Broader source proof coverage | This slice proves one accepted Bybit spot trade-replay sample/reference object and one accepted Binance BNBUSDC daily native-trades object. Registry coverage now has Bybit and Binance native-trades bindings, and the converter processed Binance's headerless single-member ZIP CSV through canonical parquet, NT catalog readback, NT `BacktestNode`, and direct S3 catalog proof with 71,431 iterations. The existing broad Binance staging manifest still contains monthly BNBUSDC trades, not the configured daily BNBUSDC object, and same-host monthly paths remain rejected under the daily binding. Generic one-object raw staging and source-proof evidence staging exist, so the missing piece for further sources is accepted proof/evidence, not converter code, manual broad backfill, or S3/SSM wiring | accept additional source proofs only after raw/schema samples are staged under S3 artifact storage, checksums/hashes are bound, license/retention evidence is recorded, and instrument-universe treatment is explicit. The preferred Binance path for broader dates/symbols is dated historical metadata; the narrower fallback is a `not_applicable` `instrument_universe` check with a matching blocking claim limit that forbids historical venue-rule/fillability/rounding/sizing/execution-quality claims. For compatible CSV native-trade sources, add proof/run-spec `[converter.csv]` mapping without changing operator/runner/NT code; if accepting monthly Binance files, add a monthly source binding/proof path rather than reusing the daily binding; for non-CSV or non-trade data, add a new registered adapter and bind its converter config hash |
| Broader NT data-query surface | The manifest now maps configured `instrument_ids` into NT `BacktestDataConfig::instrument_ids` and still declares the remaining pinned NT query surfaces as typed unsupported fields, rejects them before NT config construction, and records them in resolved-surface/result-contract claim limits. NT `instrument_ids` capability is proven by the bounded S3 fixture and now by the BTE manifest mapping tests, but query/filter/bar/time mappings are still not implemented | use configured `instrument_ids` only with explicit source-proof/fidelity implications; keep every other query knob fail-closed until it has a typed source-proof implication and a real mapping into NT; do not add ad hoc query strings, venue branches, or hidden defaults |
| Complex NT venue model policy | Primitive controls are explicit, and complex surfaces are manifest-declared with structured unsupported-surface rejection, but leverage maps, margin model, simulation modules, fill model, latency model, fee model, and settlement prices are not yet mapped into NT | keep them `unsupported_for_now` until each field has a claim-limited typed mapping into NT `BacktestVenueConfig`; do not silently rely on hidden defaults |
| Direct S3 catalog execution breadth | Direct S3 catalog execution is proven for two accepted tranches and one multi-instrument fixture: the Bybit sample/reference proof stamps `nt_iterations = 937/937`, the Binance daily proof stamps `nt_iterations = 71431/71431`, and `nt_catalog_proof` stamps `nt_instrument_count = 2`, `nt_trade_ticks = 6`, `nt_backtest_iterations = 6`, and `direct_s3_catalog_access_proven = true` | require the same proof artifact for every accepted tranche before promoting it into research analytics; do not infer direct S3 behavior from local-only runs |
| Artifact Index commit proof | The pure record and committed-discovery contracts are validated locally, and the bounded real S3 proof now proves the configured object-store path can create immutable event/snapshot/audit objects, create the first latest pointer with create-only semantics, update the latest pointer with `If-Match`/prior ETag semantics, reject a stale ETag update, and read back/resolve the final pointer and snapshot under an isolated proof root. Event/snapshot serialization is now represented by proof JSON objects and a computed canonical snapshot content hash. A follow-up S3 IAM probe proves the current generic `/bolt/artifact-store/s3/*` credential is too broad: it successfully wrote all three configured denied-kind Artifact Index paths and produced zero permission rejections. The repo now has a config-driven per-kind IAM policy generator, but the actual AWS per-kind identity/SSM parameter provisioning is not done | get explicit approval for the AWS security mutation, create per-kind producer identities/SSM parameter paths using the generated policy, then rerun the IAM-scope proof until denied-kind event/snapshot/pointer writes are rejected; if per-kind IAM is not acceptable, select an approved coordinator/table format before relying on Artifact Index commits |
| Artifact lifecycle operations | Lifecycle policy is validated in-process, including active/queryable latest-pointer/current-snapshot metadata for committed discovery. S3 bucket/object lifecycle rules, transition execution, restore behavior, and storage-class cost behavior are not proven | keep runtime deletes/expirations disabled by contract; model lifecycle costs and prove storage-class transition/restore behavior before enabling any actual bucket lifecycle policy |
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
22. Unsupported NT/run-manifest surfaces must fail before object reads and
    before derived canonical/catalog artifacts are written; otherwise a broad
    backfill can still spend time on data that was invalid from TOML alone.

## Backfill-First Status

As of the 2026-06-07 bounded S3 recheck, the overall BTE is blocked on accepted
data, not on custom simulator construction.

Current evidence:

- `s3://bolt-parquet/nt-research-analytics/` is no longer empty after the
  bounded S3 proofs. The latest recursive listing returned 37 objects totaling
  12,067,580 bytes: one accepted Binance daily raw ZIP, scratch and
  accepted-candidate Binance source-proof evidence objects, ten clean
  Bybit sample/reference backtest artifacts under
  `backtests/backtesting-vertical-slice-bnbusdc-2026-03-01/`, and ten clean
  accepted Binance backtest artifacts under
  `backtests/backtesting-vertical-slice-binance-bnbusdc-2026-03-01-accepted/`.
  The non-`-accepted` Binance backtest prefix is superseded forensic evidence
  only. This proves the SSM-backed staging/publish path for the bounded slices
  only; it is not broad historical backfill coverage.
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
- Current coverage parser normalizes 166 of those 190 manifest files into full
  count/byte evidence. The other 24 schemas are metadata-shape gaps, not
  payload gaps: Chainlink and Deribit manifests have counts/bytes but use
  `write_policy` instead of `write_mode`; Hyperliquid HIP-3/HIP-4 and one
  source-proof-v3 manifest have `write_mode` but no normalized completed-object
  and completed-byte totals.
- The supported-shape coverage ledger was generated at
  `/private/tmp/bte-coverage-ledger-20260607/ledger-output-supported/backfill-coverage-ledger.json`
  with content hash
  `ebe6363148bf53e2012bb4d013b2a8086abfdb7addcc738641a96561bd867a41`.
  It contains 166 records, all rejected: 166 `missing_source_proof`, 145
  `empty_source_binding`, 5 zero-planned/completed object/byte records, and 3
  failed-object records.
- After the unsupported-schema rejected-record path was added, the full
  190-manifest TOML census generated
  `/private/tmp/bte-coverage-ledger-20260607/ledger-output/backfill-coverage-ledger.json`
  instead of aborting. Its content hash is
  `688597378bcd1e47d49bb1e981f06a5eae0878122780320321ac3007c26dcfff`, size is
  92,091 bytes, and it contains 190 rejected records, 0 accepted records, and
  353 blocking issues: 166 `missing_source_proof`, 145 `empty_source_binding`,
  24 `unsupported_manifest_schema`, 5 `planned_objects_not_positive`, 5
  `completed_objects_not_positive`, 5 `completed_bytes_not_positive`, and 3
  `failed_objects_present`.
- Canonical source-proof reports under
  `source-proof-v3/source-proofs/v1/**/source-proof.json` total 21. The
  config-driven source-proof admissibility verifier generated
  `/private/tmp/bte-coverage-ledger-20260607/source-proof-admissibility-output/source-proof-admissibility-report.json`
  with content hash
  `2916adfc0b23a802cc01d027eebc87026debbf95c34a06a70685cdf5334ff191`.
  It contains 21 records, 0 current-contract records, 0 accept-ready records,
  and 21 non-current-contract records. Every staged proof has the same blocking
  issue classes: `missing_current_contract_field`,
  `legacy_source_binding_key_field`, `legacy_table_families_field`,
  `legacy_raw_payload_records_field`, `legacy_scalar_required_checks`, and
  `current_contract_deserialize_failed`. Every staged proof is missing these 18
  current-contract fields: `source_binding`, `product_category`,
  `table_family`, `fixture_type`, `requested_time_range`,
  `coverage_time_range`, `instrument_universe_id`, staged `raw_sample_uri` and
  `schema_sample_uri`, hashes, license/retention refs, `nt_mapping_status`,
  `fidelity_class`, `claim_limits`, `acceptance_scope`, and `gap_policy_id`.
  Registry comparison found 2 missing binding keys (`binance-coin-m-instruments`
  and `binance-usd-m-instruments`) plus 2 product-family mismatches
  (`okx-futures-instruments`, `okx-option-underlyings`). After TOML source-proof
  binding, the bound coverage
  ledger at
  `/private/tmp/bte-coverage-ledger-20260607/ledger-output-bound/backfill-coverage-ledger.json`
  has content hash
  `5f4f5e9bd991ce5508c5445d8db3e6056b7a3e91efa99fd1f19bd96c3c11708c` and
  still has 0 accepted records: 20 records are now specifically blocked by
  `source_proof_not_accepted`, while 146 records still have no source-proof
  binding.
- The config-driven legacy derivability verifier then compared those same 21
  staged source proofs against the source-proof-v3 S3 acceptance manifest and
  generated
  `/private/tmp/bte-coverage-ledger-20260607/source-proof-legacy-derivability-output/source-proof-legacy-derivability-report.json`
  with content hash
  `ce7e80b20b3290c80e7d8e434af29283ed6eef380493c01879c822be42c52e1d`.
  It contains 21 records: all 21 raw payload sets are S3/hash-bound, 19 have
  exactly one table family, and all 21 remain acceptance-blocked. Derivable
  current-contract field counts are: `source_binding` 21, `fixture_type` 21,
  `requested_time_range` 21, `coverage_time_range` 21, `acceptance_scope` 21,
  `claim_limits` 21, `table_family` 19, and single-object `raw_sample_uri` /
  `raw_sample_hash` 20. Blocking issue counts are: `license_not_passed` 21,
  `nt_mapping_not_passed` 21, `fidelity_not_passed` 21,
  `forbidden_claims_not_passed` 21, `schema_sample_not_passed` 21, and
  `not_exactly_one_table_family` 2. The two multi-table exceptions are
  `hyperliquid-hip4-outcome-meta` (`prediction_market_events`,
  `prediction_market_outcomes`) and `polymarket-parquet-archive-index`
  (`order_book_snapshots_fixed_depth`, `order_book_snapshot_deltas`, `bars`).
  Therefore source-proof migration should start with a single-table proof whose
  license, schema, fidelity, forbidden-claim, and NT mapping evidence can be
  made explicit; it should not start by broad payload conversion.
- A current-code rerun after adding aggregate summary fields generated
  `/private/tmp/bte-coverage-ledger-20260607/source-proof-legacy-derivability-current-summary-output/source-proof-legacy-derivability-report.json`
  with content hash
  `3e53e47f00259faed548d240d70a855858567556ace893c07deda5bafca39bc6`,
  size 22,653 bytes, and the same 21 source-proof records. The summary now
  materializes blocker counts directly in the artifact:
  `license_not_passed = 21`, `nt_mapping_not_passed = 21`,
  `fidelity_not_passed = 21`, `forbidden_claims_not_passed = 21`,
  `schema_sample_not_passed = 21`, and `not_exactly_one_table_family = 2`.
  It also materializes table-family counts:
  `instruments = 19`, `bars = 1`, `order_book_snapshot_deltas = 1`,
  `order_book_snapshots_fixed_depth = 1`,
  `prediction_market_events = 1`, and `prediction_market_outcomes = 1`.
  This rerun used existing source-proof and manifest artifacts only; it did
  not download raw payloads, convert rows, write an NT catalog, or accept a
  proof.
- Repo search found no checked-in source-proof-v3 generator. The new
  `source_proof_admissibility` CLI is report-only: it deserializes staged JSON
  against the current contract and calls `SourceProofReport::evaluate_acceptance`
  when deserialization succeeds, but it does not accept, migrate, or mutate
  source proofs. The existing promotion route remains the BTE operator run-spec
  path: `RunSpec.source_proof` must already deserialize as the current
  `SourceProofReport`, then `SourceProofReport::accept` stamps it and
  `select_accepted_dataset` binds exactly one hash-verified staged object. A
  manifest metadata census found only 22 of 190 manifests with top-level
  source-binding/source-proof fields; the other 168 cannot be safely bound by
  prefix or venue inference without violating the source-proof contract.
- The config-driven backfill preflight CLI was run against the current
  source-proof-bound coverage ledger before any payload conversion:
  `/private/tmp/bte-coverage-ledger-20260607/backfill-preflight-bound-output/backfill-preflight-report.json`
  has content hash
  `7e3869c002579370b8eac5e1c6b452a76ffe65e65bc53f0913afb934f61af912`,
  size 633 bytes, `status = blocked`, `total_records = 166`,
  `accepted_records = 0`, `accepted_with_gaps_records = 0`,
  `canonical_ready_records = 0`, `eligible_record_count = 0`, and blocking
  reasons `no_accepted_records` plus `no_canonical_ready_records`. This is the
  intended fail-fast point that prevents repeating the previous slow path while
  source-proof acceptance and canonical-ready evidence are still absent.
- The source-proof migration preflight CLI was run against the legacy
  derivability report with TOML-owned table-family selection. Requiring
  `allowed_table_families = ["trades"]` for the current NT `TradeTick` path
  generated
  `/private/tmp/bte-coverage-ledger-20260607/source-proof-migration-preflight-trades-output/source-proof-migration-preflight-report.json`
  with content hash
  `20f4c1a807089c1e14f171c18f993296b6d3859dce66947916364647725981ad`,
  size 871 bytes, `status = blocked`, `total_records = 21`,
  `eligible_candidate_count = 0`, no selected candidate, and blocking reason
  `no_eligible_candidate`. Allowing `allowed_table_families = ["instruments"]`
  generated
  `/private/tmp/bte-coverage-ledger-20260607/source-proof-migration-preflight-instruments-output/source-proof-migration-preflight-report.json`
  with content hash
  `d6698ad78cf0c5c858f7d5e7d69f6b85fb5e98c1ec3802415ca0b2b8a10ca18c`,
  size 1750 bytes, `status = candidate_found`, and
  `eligible_candidate_count = 19`; the selected structural candidate is
  `source-proof-f6f955810b3a6b42` / `okx-option-underlyings`, table family
  `instruments`, one S3-bound raw payload, 52 accepted bytes from S3, and the
  same remaining acceptance blockers: license, NT mapping, fidelity,
  forbidden-claim, and schema-sample checks. Therefore there is no staged
  single-table market-data `trades` proof candidate for the existing converter
  path; an instrument-universe proof can be current-contract-shaped next, but it
  still cannot be accepted until those five evidence checks are explicit.
- After derivability began preserving legacy source-proof `venue`,
  `product_family`, and `evidence_state`, and migration preflight began loading
  the spec-owned source-bindings registry, the same real proof set was rerun
  without raw payload download or conversion. The metadata derivability report
  at
  `/private/tmp/bte-coverage-ledger-20260607/source-proof-legacy-derivability-with-metadata-output/source-proof-legacy-derivability-report.json`
  has content hash
  `e86a2c6bdee28275cc9fd184bf9c24e9fef56f64506ebd528f9fa005e8743a54`,
  size 25,048 bytes, and the same 21 records. The current-registry instruments
  preflight at
  `/private/tmp/bte-coverage-ledger-20260607/source-proof-migration-preflight-instruments-with-metadata-registry-output/source-proof-migration-preflight-report.json`
  has content hash
  `a84d829d0ce6793d14e5dab88af4206c9b9d5db4db512af2be74bcf079f8eccf`,
  size 1,835 bytes, `status = candidate_found`, and
  `eligible_candidate_count = 19`; the selected `okx-option-underlyings`
  candidate now reports six remaining acceptance blockers: license, NT mapping,
  fidelity, forbidden-claim, schema-sample, and
  `source_binding_product_family_mismatch`. The current-registry trades
  preflight at
  `/private/tmp/bte-coverage-ledger-20260607/source-proof-migration-preflight-trades-with-metadata-registry-output/source-proof-migration-preflight-report.json`
  has content hash
  `fc6ffff57a63b05865d92c566459ddbe27235bf06735a7854f39e44401aa4939`,
  size 908 bytes, `status = blocked`, and `eligible_candidate_count = 0`.
  Therefore the next accepted source proof is still blocked by evidence and
  registry metadata, not by NT execution or catalog mechanics.
- After migration preflight candidate selection was changed to rank fewer
  remaining acceptance blockers before payload size, the real instruments
  preflight was rerun as an append-only scratch artifact. The acceptance-ranked
  report at
  `/private/tmp/bte-coverage-ledger-20260607/source-proof-migration-preflight-instruments-acceptance-ranked-output/source-proof-migration-preflight-report.json`
  has content hash
  `e2a2cb95004fc8edada403ea38d4a880baca8bc8cb0ebde4d61b7332af020993`,
  file SHA256
  `1df3fed20897fe45e7b52b6d83b1e281f0d0aaaa45c057cacb4eef08e838c671`,
  size 1,793 bytes, `status = candidate_found`, and
  `eligible_candidate_count = 19`. The selected candidate moved to
  `source-proof-32d52c7aa5a3910b` /
  `hyperliquid-hip3-perp-dexs`, table family `instruments`, one S3-bound raw
  payload, 14,659 accepted bytes from S3, and exactly the five unresolved
  evidence blockers: license, NT mapping, fidelity, forbidden-claim, and
  schema-sample checks. This avoids choosing either the tiny
  `okx-option-underlyings` proof with a product-family mismatch or the
  `deribit-spot-active-instruments` proof whose `bounded_or_current_only`
  evidence is not acceptable for backfill. It still does not make backfill
  ready; it only makes the no-go gate choose the most viable current candidate.
- The committed reference `trades` source proof was separately checked through
  the config-driven source-proof admissibility CLI, using
  `specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.bnbusdc-2026-03-01.json`
  as the only input. The report at
  `/private/tmp/bte-coverage-ledger-20260607/source-proof-admissibility-reference-trades-output/source-proof-admissibility-report.json`
  has content hash
  `7709e381304c1113ef9feeeecd09824a389b3526d29d05d9b15422c1fc033107`,
  size 833 bytes, one current-contract record, one accept-ready record, zero
  rejected/non-current records, and zero blocking issues. The record is
  `source-proof-bybit-spot-tick-trades` version 1 for
  `bybit-spot-tick-trades`. This proves the existing single-object sample proof
  is valid; it does not unblock broad backfill because the current coverage
  ledger still has no accepted canonical-ready record and the staged
  source-proof-v3 set still has no eligible `trades` migration candidate.
- The combined backfill readiness CLI was run for the current NT `TradeTick`
  path using the real backfill preflight report plus the real `trades`
  source-proof migration preflight report:
  `/private/tmp/bte-coverage-ledger-20260607/backfill-readiness-tradetick-output/backfill-readiness-report.json`
  has content hash
  `87a391ceda3529812d4c609ef97734c786f3d56f3cc27155c353f30ae18f0f31`,
  size 727 bytes, `status = blocked`, `required_table_family = "trades"`,
  `required_nt_data_type = "TradeTick"`, `backfill_preflight_status =
  blocked`, `source_proof_migration_preflight_status = blocked`, no selected
  backfill record, no selected source-proof candidate, and four blockers:
  `backfill_preflight_blocked`, `source_proof_migration_preflight_blocked`,
  `missing_selected_backfill_record`, and
  `missing_selected_source_proof_candidate`. This is the current end-to-end
  no-go artifact for the existing TradeTick backfill path.
- After source-binding coverage became part of combined readiness, the same
  TradeTick path was rerun with the real strict binding-coverage report at
  `/private/tmp/bte-coverage-ledger-20260607/backfill-readiness-tradetick-with-binding-output/backfill-readiness-report.json`.
  The report content hash is
  `659901841f7e95d6740e6c3ec1d928ea91a42151a595848ffe9842cc0ce1aab2`
  and file SHA256 is
  `fced283f2966456be401cc3caf924dfc9c0bcb837240fcc64a6f077a35c95e6d`.
  It remains `blocked`; the joined statuses are backfill preflight `blocked`,
  source-proof migration preflight `blocked`, and binding coverage `blocked`,
  with blockers `backfill_preflight_blocked`,
  `source_proof_migration_preflight_blocked`,
  `backfill_binding_coverage_blocked`, `missing_selected_backfill_record`,
  and `missing_selected_source_proof_candidate`.
- After selected source-binding consistency checks were added, rerunning the
  same integrated readiness spec preserved the same content hash
  `659901841f7e95d6740e6c3ec1d928ea91a42151a595848ffe9842cc0ce1aab2`,
  byte count `911`, status `blocked`, and five blockers because the current
  real reports still have no selected backfill record or selected source-proof
  candidate to compare.
- After selected source-proof id/version consistency checks were added,
  rerunning the same integrated readiness spec again preserved content hash
  `659901841f7e95d6740e6c3ec1d928ea91a42151a595848ffe9842cc0ce1aab2`,
  byte count `911`, status `blocked`, and five blockers for the same reason:
  current real reports still have no selected backfill record or selected
  source-proof candidate to compare.
- After selected binding coverage was tightened to require accepted and
  canonical-ready counts, rerunning the same integrated readiness spec again
  preserved content hash
  `659901841f7e95d6740e6c3ec1d928ea91a42151a595848ffe9842cc0ce1aab2`,
  byte count `911`, status `blocked`, and five blockers because the real
  TradeTick path is already blocked before any selected binding exists.
- After selected binding coverage was tightened to require the selected
  binding's own table-family list to contain the readiness-required table
  family, rerunning the same integrated readiness spec again preserved content
  hash `659901841f7e95d6740e6c3ec1d928ea91a42151a595848ffe9842cc0ce1aab2`,
  byte count `911`, status `blocked`, and five blockers. This closes the
  artifact-pairing false-ready case without changing the current real no-go
  artifact.
- After combined readiness was tightened to block selected source-proof
  migration candidates with remaining acceptance blockers, rerunning the same
  integrated readiness spec again preserved content hash
  `659901841f7e95d6740e6c3ec1d928ea91a42151a595848ffe9842cc0ce1aab2`,
  byte count `911`, status `blocked`, and five blockers because the real
  TradeTick path still has no selected source-proof candidate.
- After combined readiness was tightened to require the backfill-preflight
  artifact's own selection to have `require_canonical_ready = true`, rerunning
  the same integrated readiness spec again preserved content hash
  `659901841f7e95d6740e6c3ec1d928ea91a42151a595848ffe9842cc0ce1aab2`,
  byte count `911`, status `blocked`, and five blockers.
- After combined readiness was tightened to require the selected backfill
  record itself to be canonical-ready, rerunning the same integrated readiness
  spec again preserved content hash
  `659901841f7e95d6740e6c3ec1d928ea91a42151a595848ffe9842cc0ce1aab2`,
  byte count `911`, status `blocked`, and five blockers.
- After combined readiness was tightened to require TOML-owned
  `supported_data_paths`, the integrated readiness spec was rerun with
  `supported_data_paths = [{ table_family = "trades", nt_data_type =
  "TradeTick" }]` into
  `/private/tmp/bte-coverage-ledger-20260607/backfill-readiness-tradetick-with-binding-supported-path-output/backfill-readiness-report.json`.
  The report content hash is
  `627d528ee0dc9b30280c824ab08aa3580dfe1850c9bd1abc02f4f38275a5bb1c`,
  byte count `1037`, status `blocked`, and five blockers. The report records
  `supported_data_paths`, so the supported data path is now an artifact fact,
  not a production-code table-family literal.
- The source-binding coverage CLI was run against the committed
  `backfill-source-bindings.v1.toml` registry and the current source-proof-bound
  coverage ledger for `required_table_families = ["trades"]`:
  `/private/tmp/bte-coverage-ledger-20260607/backfill-binding-coverage-trades-output/backfill-binding-coverage-report.json`
  has content hash
  `f50b38f3c19517ab5937f992ca700b941b311f657035cb92a8e8b8468f0d9067`,
  size 7211 bytes, `status = blocked`,
  `configured_required_binding_count = 2`, and
  `ledger_records_for_required_bindings = 0`. The two configured `trades`
  bindings are `binance-spot-native-trades` and `bybit-spot-tick-trades`; both
  have zero ledger records, zero canonical-ready records, and zero accepted
  records. The same report found 145 ledger records with empty source binding
  and one unconfigured binding key, `bybit:rest+public_archive:v5`. Therefore
  the next backfill unblocker is not converter execution: current manifests must
  be safely bound to configured source-binding keys and source proofs before any
  broad canonical conversion can be selected.
- After the stricter binding-coverage gate, the same inputs were rerun into
  `/private/tmp/bte-coverage-ledger-20260607/backfill-binding-coverage-trades-strict-output/backfill-binding-coverage-report.json`.
  The report content hash is
  `57a78ad13ab347f761878e4707a1ff2b8305bade490877b3a488d41f7cb46291`
  and file SHA256 is
  `d9957c998be25255c8974d28570e10e0ca5774e947c91dc2a44a5943db606c0d`.
  It remains `blocked` with `configured_required_binding_count = 2`,
  `ledger_records_for_required_bindings = 0`,
  `empty_source_binding_record_count = 145`, unconfigured binding
  `bybit:rest+public_archive:v5`, and blockers
  `no_ledger_records_for_required_table_family`,
  `empty_source_binding_records`, and `unconfigured_source_binding_records`.
- The source-proof scope CLI was run against the accepted reference trades
  proof and the matching raw staging manifest:
  `/private/tmp/bte-coverage-ledger-20260607/backfill-source-proof-scope-reference-trades-output/backfill-source-proof-scope-report.json`
  has content hash
  `db110e9baf7a6bf710cbb35387424b3867404f79a4afd022cf85917c8b910e3b`,
  size 1092 bytes, `status = candidate_found`, source proof
  `source-proof-bybit-spot-tick-trades` version 1, source binding
  `bybit-spot-tick-trades`, and manifest
  `bybit-backfill-run-fdcc0758bbd03113`. The accepted proof scope is one
  object and 8505 bytes; the enclosing manifest has 49 payload objects. The
  report found exactly one matching object, the accepted
  `s3://bolt-parquet/backfill-staging/2026-06-01/bybit/raw/v1/source=public_archive/family=tick_trades/category=spot/dt=2026-03-01/symbol=BNBUSDC/object=d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598.csv.gz`,
  and `object_level_tranche_required = true`. Therefore the safe next backfill
  unit is an object-level tranche selected by source-proof raw sample identity,
  not a run-level binding of the 49-object manifest.
- The accepted-tranche CLI was run against that source-proof scope report:
  `/private/tmp/bte-coverage-ledger-20260607/backfill-accepted-tranche-reference-trades-output/backfill-accepted-tranche-manifest.json`
  has content hash
  `90078dfb15f2056b122ced643a8072d088ea9d69b3b0e0afaf73d7b941b95c26`,
  size 1146 bytes, `status = accepted`, source-proof scope report hash
  `db110e9baf7a6bf710cbb35387424b3867404f79a4afd022cf85917c8b910e3b`,
  source proof `source-proof-bybit-spot-tick-trades` version 1, source binding
  `bybit-spot-tick-trades`, parent manifest
  `bybit-backfill-run-fdcc0758bbd03113`, `object_count = 1`, and
  `accepted_bytes = 8505`. The sole object is the same accepted raw sample hash
  `d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598`.
  This is the first machine-readable accepted object-level raw tranche for the
  reference trades path; it still requires normalization, gap/instrument policy,
  NT catalog projection, and BacktestNode/result-contract verification before
  broader BTE confidence can be claimed.
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

1. Local single-object vertical slice: proven for the accepted reference
   tranche. It has a tested accepted sample proof, generic run/proof/result
   boundaries, NT catalog projection/read-back, and NT execution through
   `BacktestNode` against the accepted object.
2. Backfill foundation: not complete. There is substantial staged raw data and
   a generic coverage-ledger/parser/aggregate plus local idempotent artifact
   writer, batch/local-file manifest-summary ingestion boundaries, a TOML
   coverage spec, an operator CLI for that spec, generic TOML source-proof
   metadata binding, unsupported-schema rejected records, and a report-only
   source-proof admissibility CLI plus a report-only legacy source-proof
   derivability CLI plus a preflight selector that refuses broad conversion
   unless the ledger has one bounded canonical-ready accepted tranche, and a
   source-proof migration preflight selector that proves whether the staged
   legacy proofs contain a candidate for a requested table family, ranks
   candidates by remaining acceptance blockers before payload size, and treats
   non-backfillable evidence states as blockers, and a combined readiness report
   that joins both gates for the selected NT data path, source-binding coverage
   over the registry and ledger, and object-level
   source-proof scope coverage proving the accepted sample is inside a broader
   raw staging manifest, plus an accepted object-level raw tranche manifest for
   that sample. A real
   manifest-only ledger can now be generated across all 190 observed manifests,
   and real source-proof admissibility/derivability/preflight reports can now
   be generated across the current staged evidence. The
   current S3 evidence still produces rejected gates, not accepted backfill: all
   discovered source-proof reports are non-current-contract source-proof-v3
   records, all 21 still lack passed license/schema/fidelity/forbidden-claim/NT
   mapping checks, 24 manifest files still need count/byte normalization if they
   are to carry detailed coverage evidence, and 146 supported manifest records
   still lack source-proof binding. There are no accepted normalized row tables,
   no accepted instrument/gap policy ledger, and no NT catalog export from that
   data.
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
3. Normalize the 24 unsupported manifest schemas into detailed count/byte
   evidence only if their source proofs become admissible; until then, keep them
   as `unsupported_manifest_schema` rejected coverage records.
4. Reconcile physical-only S3 objects, starting with PMXT and Bybit orphan
   acceptance manifests, into accepted or rejected coverage records.
5. The accepted reference trades tranche has now been normalized into the
   declared trade table contract and exported to an NT catalog locally; repeat
   this only from ledger-selected, source-proof-bound execution plans, not from
   venue names or raw prefixes.
6. Production BTE should run only after the next tranche has the same source
   proof, normalized-row, gap-policy, NT catalog, and result-contract proof.

Midpoint table-family coverage audit:

- Root cause found: binding coverage counted ledger records by
  `source_binding` only, while the registry allows source bindings to declare
  multiple `table_families`. A multi-family binding could therefore make a
  requested table family look covered with an unscoped ledger record.
- Fix applied in the vertical slice: `BackfillCoverageRecord` now carries
  optional `table_family`, and binding coverage counts by
  `(source_binding, table_family)`. Records with source binding but no table
  family remain readable as legacy evidence, but they no longer prove
  table-family readiness and produce `missing_table_family_records`.
- RED proof: `binding_coverage_blocks_unscoped_records_for_multi_family_binding`
  previously returned `Ready`; it now blocks unscoped multi-family evidence.
- Real strict binding-coverage proof remains blocked:
  `/private/tmp/bte-coverage-ledger-20260607/backfill-binding-coverage-trades-strict-table-family-output/backfill-binding-coverage-report.json`
  has content hash
  `5c3ae7462f7989b5092331ff6c40a03b9c96462e76df3eff5ef0ee9c77c8e4ef`,
  status `blocked`, `ledger_records_for_required_bindings = 0`,
  `missing_table_family_record_count = 21`,
  `empty_source_binding_record_count = 145`, and unconfigured source binding
  `bybit:rest+public_archive:v5`.
- Follow-up root cause: raw object path segment `family=<value>` is not
  consistently the canonical table family (`tick_trades` versus `trades`,
  `instrument_universe` versus `instruments`), so deriving table family from S3
  paths would recreate venue/data-family assumptions.
- Generic fix applied: the manifest TOML override path that already binds
  `source_binding` and `source_proof_id` now also binds `table_family`. One
  PMXT single-manifest proof produced ledger hash
  `3bce9e338a1976afc2ebd3ae1958a2678320ed2b1358a1aa1a6dee2473f578b9`
  with `table_family = "order_book_snapshots_fixed_depth"` and source proof
  still rejected as pending. Binding coverage over that one-manifest ledger
  produced hash
  `f3a8197f8bc6ba573662791e7bd3428a4188500cce00be86af8abcbda59b5d5a`,
  status `ready`, `ledger_records_for_required_bindings = 1`, and
  `accepted_record_count = 0`, proving source-binding coverage is now
  table-family scoped while source-proof readiness remains a separate blocker.
- Object-level reference trades scope and accepted tranche now preserve the
  canonical table family from the accepted source proof. Fresh source-proof
  scope proof
  `/private/tmp/bte-coverage-ledger-20260607/backfill-source-proof-scope-reference-trades-table-family-output/backfill-source-proof-scope-report.json`
  produced hash
  `4c62c9d7cd2efa71e07ac83344989820e1c9b2dfc13a42f423c628777bdd18c3`,
  status `candidate_found`, `matching_object_count = 1`,
  `object_level_tranche_required = true`, and `table_family = "trades"`.
  The accepted tranche generated from it produced hash
  `81782b053ac9adbc9be156adabd8b40f9c602a5b05e940626afa4f0dced8251f`,
  status `accepted`, `object_count = 1`, `accepted_bytes = 8505`, and
  `table_family = "trades"`.
- The execution-plan handoff generated from that table-family-aware tranche
  produced
  `/private/tmp/bte-coverage-ledger-20260607/backfill-execution-plan-reference-trades-table-family-output/backfill-execution-plan.json`
  with hash
  `3ee2fbb4bcc3ce42d204692a6882882aa1dc039f4362e40b78be6b2f0c263297`,
  status `ready`, `object_count = 1`, `accepted_bytes = 8505`, and
  `table_family = "trades"`. This keeps the canonical table family attached
  through the last pre-fetch execution gate; the broader manifest-level
  backfill preflight remains blocked until accepted canonical-ready coverage
  records exist for non-sample tranches.
- Follow-up enforcement fix: execution-plan generation and CLI pre-fetch
  validation now reject table-family mismatches between the accepted tranche
  and submitted run spec before object reads. The reference execution plan
  rerun remained deterministic with hash
  `3ee2fbb4bcc3ce42d204692a6882882aa1dc039f4362e40b78be6b2f0c263297`;
  focused tests now cover both evaluator-level
  `RunSpecTableFamilyMismatch` and CLI rejection before object-reader
  invocation.
- Follow-up readiness fix: backfill preflight selected records now preserve
  `table_family`, and combined readiness rejects a selected canonical-ready
  backfill record whose own table family differs from the requested supported
  path. This closes the remaining indirect-inference gap where readiness could
  rely on source binding and separate binding coverage without proving the
  selected preflight record itself was for the requested data family.
- Current-code real manifest-only rerun: `backfill_preflight` over
  `/private/tmp/bte-coverage-ledger-20260607/backfill-preflight-bound.toml`
  remained blocked with `eligible_record_count = 0`, content hash
  `7e3869c002579370b8eac5e1c6b452a76ffe65e65bc53f0913afb934f61af912`;
  joined `backfill_readiness` over the TradeTick supported path remained
  blocked with five blockers, content hash
  `627d528ee0dc9b30280c824ab08aa3580dfe1850c9bd1abc02f4f38275a5bb1c`.
  These reruns used existing manifest/report artifacts only, not raw payload
  downloads or conversion.
- Current-head rerun on 2026-06-08 kept the same efficient no-go boundary:
  `backfill_preflight` selected no record from 166 manifest-ledger records
  (`no_accepted_records`, `no_canonical_ready_records`), `source_proof_migration_preflight`
  selected no `trades` candidate from 21 legacy source-proof records
  (`no_eligible_candidate`), current-branch binding coverage found zero ledger
  records for required `trades` bindings, and joined `backfill_readiness`
  remained blocked with `backfill_preflight_blocked`,
  `source_proof_migration_preflight_blocked`,
  `backfill_binding_coverage_blocked`, `missing_selected_backfill_record`,
  and `missing_selected_source_proof_candidate`. The refreshed reports were
  written under `/private/tmp/bte-current-head-preflight-20260608/` and did not
  download, decode, convert, or publish raw payloads.
- Follow-up fail-fast fix: unsupported run-manifest/catalog surfaces now reject
  before object reads and before canonical normalization writes. The RED test
  proved the previous CLI path invoked the object reader for an unsupported
  catalog data type; GREEN verification now covers the CLI preflight plus
  fresh and completed-output operator paths.
- Binary-option source-proof population is now machine-readable before provider
  selection. RED
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_source_proof_reference_fixtures -- --nocapture`
  failed because no `source-proof-fixture.*.json` reports were committed.
  GREEN passed after adding
  `specs/023-nt-research-analytics-platform/reference/source-proof-fixture.binary-option.official-free-pending.v1.json`.
  The report records the TOML-selected official/free binary-option candidate as
  `status = pending`, `source_selection_status = PENDING_MORE_PROOF`,
  `fidelity_class = METADATA_ONLY`, and `evidence_state =
  bounded_or_current_only`; it carries no `acceptance_scope`, no acceptance
  provenance, no raw/catalog/result payloads, and explicit claim limits
  forbidding NT catalog/backtest input, historical replay claims, L2/L3
  execution-quality claims, and quote-token/NT BinaryOption mapping claims
  until those proofs pass.
- Perps/spot source-proof population is now also machine-readable before
  provider selection. RED on the same focused integration test failed with
  `perps/spot fixture needs a SourceProofReport before provider selection`.
  GREEN passed after adding
  `specs/023-nt-research-analytics-platform/reference/source-proof-fixture.perps-spot.official-free-pending.v1.json`.
  The report records the TOML-selected official/free native-trades candidate as
  `status = pending`, `source_selection_status = PENDING_MORE_PROOF`,
  `fidelity_class = TRADE_REPLAY`, and no accepted object scope or acceptance
  provenance. It blocks NT catalog/backtest input, provider-selected production
  source claims, L2/L3/execution-quality claims from trade-print replay, and
  broad one-year/multi-instrument coverage claims until the source proof,
  license, sample, NT mapping, fidelity, and cost checks pass. The reference
  fixture test also enforces that any future `product_category =
  kimchi-premium` fixture carries point-in-time `korean_spot`,
  `reference_price`, `fx_quote`, and `token_mapping` component source-proof
  roles; no kimchi-premium source family is selected in the current reference
  fixtures.
- Source shortlisting is now current-report based, not prose, venue examples,
  or legacy derivability selection. RED
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_source_proof_shortlist -- --nocapture`
  first failed because `source_proof_shortlist` did not exist, then failed for
  the missing TOML/file writer path. GREEN passed after adding
  `crates/backtesting-vertical-slice/src/source_proof_shortlist.rs` and
  `src/bin/source_proof_shortlist.rs`. The gate accepts only typed
  `SourceProofReport` inputs, filters by fixture, table family, and candidate
  class, excludes rejected reports, carries remaining required-check blockers
  forward for pending candidates, writes an idempotent
  `source-proof-shortlist-report.json`, and rejects legacy/prose JSON because
  it cannot deserialize as `SourceProofReport`. Verification also reran the
  BTE-016/017 reference fixture test and compiled the new bin; targeted clippy
  passed for `--lib --bin source_proof_shortlist`.
- Canonical and backtest input now require an already accepted
  `SourceProofReport`; the operator no longer accepts a pending proof as part
  of a run. RED
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --lib run_from_run_spec_rejects_pending_source_proof_before_canonical_work -- --nocapture`
  first failed because the pending run-spec reached NT work. GREEN moved
  acceptance to committed run-spec/proof artifacts, verifies proof provenance
  matches the run-spec, and rejects pending proofs before conversion
  checkpoint, canonical Parquet, or NT catalog writes. Both committed
  reference run-specs now carry the same accepted source-proof provenance as
  their accepted proof JSON artifacts, and the targeted operator suite
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --lib operator::tests -- --nocapture`
  passed with 32 tests.
- License/commercial-use boundaries are now machine-readable in
  `SourceProofReport` through `license_scope`. Accepted BTE catalog/backtest
  input rejects `unknown` and personal-use-only scopes; public, commercial,
  enterprise, or waived scopes are allowed when the existing license required
  check also passes with evidence. Current accepted Bybit and Binance reference
  proofs/run-specs are explicitly `license_scope = public`; pending fixture
  reports stay `unknown`. Focused verification passed:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --lib source_proof::tests -- --nocapture`
  (74 tests),
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_source_proof_reference_fixtures -- --nocapture`
  (2 tests), source-proof admissibility plus backfill source-proof scope tests
  (7 tests), and `operator::tests` (32 tests).
- Representative sample inspection is now bound to the shortlisted source-proof
  fixtures without starting broad backfill. The binary-option fixture records a
  bounded current Hyperliquid `outcomeMeta` probe captured at
  `2026-06-07T19:59:21Z`: 21,068 response bytes, response SHA256
  `2a9884457e2a3118e7497a7d41dee840846526fcd50d43c0e8cfd262be67c846`, 62
  outcome rows, and three question rows, summarized in
  `specs/023-nt-research-analytics-platform/reference/source-proof-sample-inspection.hyperliquid-hip4-outcome-meta.2026-06-08.json`.
  The perps/spot fixture binds to the already accepted Binance BNBUSDC
  2026-03-01 raw/schema sample hashes instead of a new broad download. Both
  fixtures remain `status = pending` and continue blocking canonical NT
  catalog/backtest input until license, retention, coverage, storage,
  parser/mapping, fidelity, and cost checks pass. The reference-fixture test now
  enforces this provider-agnostically: every committed `source-proof-fixture.*`
  report must bind non-placeholder raw/schema sample URIs and hashes and must
  mark source-access and schema sample checks as passed before provider
  selection. Focused verification passed:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_source_proof_reference_fixtures -- --nocapture`
  (2 tests).
- BTE-022 is split by actual NT-compatible data class, not by venue. Pinned NT
  exposes `BinaryOption` as an `InstrumentAny` catalog instrument and can write
  instrument records through `ParquetDataCatalog::write_instruments`, but
  `BacktestDataConfig.data_type` dispatches replay inputs through market-data
  classes such as `TradeTick`, `QuoteTick`, `Bar`, `OrderBookDelta`, and
  `OrderBookDepth10`. The current binary-option sample is Hyperliquid outcome
  metadata only, so it remains `METADATA_ONLY`, `nt_mapping_status =
  not_applicable`, and blocked from NT catalog/backtest claims. The committed
  mapping inspection report
  `specs/023-nt-research-analytics-platform/reference/source-proof-nt-mapping-inspection.hyperliquid-hip4-outcome-meta.2026-06-08.json`
  records why: `outcomeMeta` has outcome labels, side specs, quote token, and
  question metadata, but not the activation/expiration timestamps or price/size
  increments required for checked NT `BinaryOption` construction. A bounded
  current probe for outcome side `#1010` showed `l2Book` and recent
  `candleSnapshot` shapes can exist, but those probes are not accepted
  historical coverage or a replay data source. The perps/spot native-trades
  fixture now carries the already proven NT `TradeTick` mapping:
  `nt_mapping_status = accepted`, `required_checks.nt_mapping.outcome =
  passed`, and evidence bound to the accepted Binance BNBUSDC 2026-03-01 sample
  plus `ParquetDataCatalog` projection/query read-back. RED/GREEN focused test
  evidence: the reference-fixture test first failed because the perps/spot
  fixture still had pending NT mapping, then failed because the metadata-only
  binary fixture lacked a committed NT mapping inspection, then passed after
  binding both facts. BTE-022 remains open overall until the binary-option
  fixture either proves replay data into an NT data class or is explicitly
  routed through an approved non-replay signal/input contract.
- PMXT/Polymarket is now recorded as the binary-option replay candidate without
  making it selected or accepted. The committed sample inspection
  `specs/023-nt-research-analytics-platform/reference/source-proof-sample-inspection.polymarket-pmxt-v2-orderbook.2026-06-08.json`
  binds the public PMXT v2 hourly parquet source
  `https://r2v2.pmxt.dev/polymarket_orderbook_2026-05-20T22.parquet`,
  HEAD metadata, local bounded SHA256
  `0de44455fde7aedd6678fa30cc1ef86ba215eaf70fb3f7b9735510e1371f6567`,
  and schema/event counts: `price_change`, `book`, `last_trade_price`, and
  `tick_size_change` over the `2026-05-20T22:00Z` archive hour. Public source
  docs identify the archive as hourly Polymarket CLOB market-channel parquet,
  public HTTPS with no credentials, and CC BY 4.0 including commercial use with
  attribution. The pending source-proof fixture
  `source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json`
  therefore marks source access, license, schema, time semantics, and
  granularity as passed, but leaves instrument universe, coverage,
  retention/freshness, completeness, NT mapping, storage, and cost pending.
- The PMXT NT mapping inspection
  `specs/023-nt-research-analytics-platform/reference/source-proof-nt-mapping-inspection.polymarket-pmxt-v2-orderbook.2026-06-08.json`
  records the exact blocker: PMXT fields can feed NT `OrderBookDelta` and
  `TradeTick`, and a bounded local prototype catalog physically wrote
  `InstrumentAny::BinaryOption`, `OrderBookDelta`, and `TradeTick` parquet, but
  that prototype is not acceptable evidence because the instrument rows used
  placeholder `activation_ns`, `expiration_ns`, and outcome metadata. The
  generic reference-fixture test now enforces that any `L2_REPLAY` fixture must
  bind replay evidence and either carry accepted `OrderBookDelta` or
  `OrderBookDepth10` plus `ParquetDataCatalog` readback, or remain pending with
  a committed mapping inspection and a source-backed `BinaryOption` blocker.
  Focused verification passed:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_source_proof_reference_fixtures -- --nocapture`
  (2 tests).
- A bounded official PMXT/Polymarket instrument join now narrows the binary
  option mapping blocker without accepting it. The committed inspection
  `specs/023-nt-research-analytics-platform/reference/source-proof-nt-mapping-inspection.polymarket-pmxt-v2-orderbook-instrument-join.2026-06-08.json`
  hash
  `f4c5353a8f19ccdc2c0a86feefa0f74bdc929bea82e3abd384f0bf73aea1f8f8`
  binds the sampled PMXT token
  `44554681108074793313893626424278471150091658237406724818592366780413111952248`
  to official CLOB/Gamma metadata for condition
  `0x00000977017fa72fb6b1908ae694000d3b51f442c2552656b10bdbbfd16ff707`.
  The official probes source-back the sampled token outcome (`Yes`), twin token
  (`No`), price increment/minimum tick (`0.001`), minimum order size (`5`),
  accepting-orders timestamp (`2025-11-06T15:54:33Z`), and Gamma end date
  (`2026-12-31T00:00:00Z`). Pinned NT source inspection then shows the
  non-custom mapping path already exists in
  `nautilus-polymarket`: `PolymarketInstrumentProvider` indexes Gamma-loaded
  instruments by token id, `parse_gamma_market` derives the instrument
  definition from Gamma fields, `create_instrument_from_def` constructs
  `InstrumentAny::BinaryOption` via `BinaryOption::new_checked`, and
  `rebuild_instrument_with_tick_size` handles tick-size changes while
  preserving instrument fields. That NT path resolves currency (`pUSD`),
  `size_precision = 6`, `size_increment = 0.000001`, activation from Gamma
  `startDate`, expiration from Gamma `endDate`, and fee defaults/schedule
  semantics. Those modules and parser/provider items are public in the pinned
  `nautilus-polymarket` crate, so direct reuse is blocked by BTE dependency
  wiring rather than by an absent NT API. The exception is the historical
  `build_polymarket_trade_id` helper in the NT HTTP data API path: it is
  `pub(crate)`, so the selected PMXT trade-id path needs a tested local mirror
  of the pinned format or an upstream/public NT surface. The fixture remains
  pending because the isolated BTE crate does not yet have an approved
  `nautilus-polymarket` dependency/lock entry and still needs a generic PMXT
  order-book adapter plus `ParquetDataCatalog` write/read-back.
- Kalshi official historical endpoints are now verified and downgraded for BTE
  source selection. The committed endpoint inspection
  `specs/023-nt-research-analytics-platform/reference/source-proof-endpoint-inspection.kalshi-official-historical-api.2026-06-08.json`
  binds official docs and bounded public API probes. Kalshi historical docs list
  only `GET /historical/cutoff`, `GET /historical/markets`,
  `GET /historical/markets/{ticker}`,
  `GET /historical/markets/{ticker}/candlesticks`,
  `GET /historical/trades`, `GET /historical/fills`, and
  `GET /historical/orders`; current/live orderbook surfaces are
  `GET /markets/{ticker}/orderbook`, `GET /markets/orderbooks`, and
  authenticated WebSocket `orderbook_snapshot`/`orderbook_delta` updates.
  A public cutoff probe returned `2026-04-08T00:00:00Z` for market, trade, and
  order historical cutoffs; a historical market/trade/candlestick sample was
  reachable; and a checked historical-orderbook path returned `404`. The new
  pending fixture
  `source-proof-fixture.binary-option.kalshi-official-historical-pending.v1.json`
  records `fidelity_class = TRADE_BAR_REPLAY`, passes source-access/schema/time
  semantics/granularity, keeps license/instrument-universe/coverage/
  retention/completeness/NT mapping/storage/cost pending, and forbids
  historical L2/L3 order-book replay and execution-quality claims. RED/GREEN
  evidence: `source_proof::tests::committed_registry_exposes_required_market_structure_fixtures`
  first failed because `kalshi-official-historical-api` was absent from the
  committed registry, then passed after adding the TOML-owned binding with
  `bars`, `trades`, and `prediction_market_events` table families and no
  `order_book_deltas` claim. The reference-fixture test also passed with the
  new Kalshi pending proof.
- BTE-024 is now closed for the committed shortlisted fixture set. Current
  fixture evidence is:
  `hyperliquid-hip4-outcome-meta` = `METADATA_ONLY`,
  `polymarket-parquet-archive-index` = `L2_REPLAY`,
  `kalshi-official-historical-api` = `TRADE_BAR_REPLAY`, and
  `binance-spot-native-trades` = `TRADE_REPLAY`. Each fixture carries four
  forbidden claims and four structured claim-limit records. The common blocking
  claim across all pending fixture records is no canonical NT catalog/backtest
  input before acceptance; weaker-fidelity records explicitly forbid historical
  L2/L3, execution-quality, queue-position, fillability, liquidity, or sizing
  claims as applicable. The audit command used was:
  `jq -r '[input_filename, .source_binding, .fixture_type, .table_family, .fidelity_class, .nt_mapping_status, (.forbidden_claims|length), (.claim_limits|length), (.required_checks.nt_mapping.outcome // "missing")] | @tsv' specs/023-nt-research-analytics-platform/reference/source-proof-fixture.*.json`.
- BTE-026 now has a bounded cost estimate artifact:
  `specs/023-nt-research-analytics-platform/reference/source-proof-cost-estimate.backtesting-engine.2026-06-08.json`.
  It records official/current AWS S3, Athena, CloudWatch Logs, data transfer,
  Fargate, and Batch pricing evidence, plus provider subscription facts for
  PMXT, Binance public data, Kalshi historical APIs, Hyperliquid requester-pays
  archive, Tardis Perpetuals Professional, and Telonex Plus. The important
  backfill-risk finding is that the current PMXT Polymarket v2 index averages
  about `514 MB/hour`, or roughly `12 GiB/day` and `4.29 TiB/year`, before
  conversion, NT catalog writes, validation scans, or downstream analytics. A
  one-year PMXT Polymarket v2 planning scenario is about `$101/month` in S3
  Standard storage, `$15.83/month` in Glacier Flexible Retrieval, `$4.35/month`
  in deep archive storage, `$21.47` per full Athena scan, and about `$94.11`
  for a 100-hour `16 vCPU`/`64 GB` Fargate conversion worker baseline. This
  closes the planning estimate task only; fixture-level cost checks remain
  pending until the exact source binding, date window, instrument universe,
  coverage ledger, and throughput benchmark are accepted.
- BTE-025 now has a highest-fidelity evaluation artifact:
  `specs/023-nt-research-analytics-platform/reference/source-proof-fidelity-evaluation.backtesting-engine.2026-06-08.json`.
  It closes only the planning evaluation task, not source selection or broad
  backfill authorization. The result is: Binance official public archive remains
  lower-fidelity trade replay while Tardis Binance L2 is a pending paid/vendor
  candidate; PMXT Polymarket is now scoped only to one-off selected-source
  bootstrap/projection evidence and must not block durable Polymarket source
  selection; PMXT Kalshi is likewise one-off evidence rather than a standing
  blocker for official Kalshi lower-fidelity claim-limited paths; and
  Hyperliquid HIP-4 historical
  execution-quality replay remains unproven because current outcomeMeta/current
  book probes and generic Hyperliquid L2 metadata do not bind exact historical
  outcome replay coverage. The artifact also records slow-backfill controls:
  source-proof and coverage-ledger gates before download, index/object byte
  estimates before payload pulls, sample-first schema inspection, bounded
  fixture/source/time/object/byte budgets, idempotent manifests, and no source
  choice by venue name.
- BTE-028 now has an explicit cost-cut lever status artifact:
  `specs/023-nt-research-analytics-platform/reference/source-proof-cost-cut-levers-status.backtesting-engine-028.2026-06-08.json`.
  It closes only the planning-control task, not source selection, NT mapping, or
  broad backfill authorization. The approved levers are coverage-ledger gating
  before payload download, a one-object PMXT Polymarket proof cap with
  `usage_scope = one_off_backfill_data`, source-proof acceptance before conversion, NT-native
  catalog projection/read-back as the expansion gate, bounded validation
  queries, and lifecycle placement after proof. It explicitly forbids treating
  cheaper lower-fidelity data, venue/provider hardcodes, local prototypes,
  skipped NT read-back, or broad-history-first downloads as valid cost cuts.
- BTE-022 now has an explicit open mapping-status artifact:
  `specs/023-nt-research-analytics-platform/reference/source-proof-nt-catalog-mapping-evaluation.backtesting-engine.2026-06-08.json`.
  It does not mark BTE-022 complete. It records that the only currently accepted
  sample-to-NT path is the native-trades `TradeTick` path already proven through
  `ParquetDataCatalog` write/read-back. NT itself supports `QuoteTick`,
  `TradeTick`, `Bar`, `OrderBookDelta`, `OrderBookDepth10`,
  `MarkPriceUpdate`, `IndexPriceUpdate`, `InstrumentStatus`, and
  `InstrumentClose`; the current BTE manifest now admits `TradeTick` and
  `OrderBookDelta` for typed NT data-config construction, and
  `catalog_projection.rs` has NT-native fixture proof for `BinaryOption`,
  `OrderBookDelta`, and `TradeTick` catalog write/read/logical-hash. Other data
  classes must continue to fail closed until typed projection and catalog
  read-back proof exist. PMXT Polymarket is only a one-off selected-source
  input for proving `OrderBookDelta` and `TradeTick` catalog plumbing; the
  durable path still requires BTE to reuse the pinned NT Polymarket
  parser/provider for instruments instead of inventing a custom BinaryOption
  mapper. Current dependency wiring now machine-checks the isolated
  `crates/backtesting-vertical-slice` boundary against `nautilus-polymarket`
  public surfaces, but selected PMXT raw-row-to-NT projection and catalog
  read-back remain unproven. Kalshi official history remains lower-fidelity
  `Bar` or `TradeTick` candidate only; PMXT Kalshi L2 is one-off evidence, not
  a standing blocker, and would still need source-backed BinaryOption fields,
  timestamp policy, and NT catalog
  read-back before any mapping claim; HIP-4 remains metadata-only until exact
  historical outcome replay coverage and checked BinaryOption fields are
  source-backed.

## 2026-06-08 PMXT first-proof selector midpoint

The bounded PMXT Polymarket first-proof selector now has a concrete
source-backed selector artifact, but it is still not a PMXT-to-NT catalog
projection or BacktestNode proof.

- Source object:
  `/private/tmp/polymarket-may20-one.parquet`, source URI
  `https://r2v2.pmxt.dev/polymarket_orderbook_2026-05-20T22.parquet`, SHA-256
  `0de44455fde7aedd6678fa30cc1ef86ba215eaf70fb3f7b9735510e1371f6567`.
- Event-count ledger:
  `/private/tmp/bte-pmxt-first-proof-selector-2026-06-08/event-count-ledger.json`;
  `source_rows = 64877467`, `event_count_rows = 91786`, logical content hash
  `5a51219a0988271c4f648d56341d3fadc734a31af2801fd957100dd513e9c3a6`, file
  SHA-256 `de29c2edd18f4a23d24c42a90ad379b7ec602ea83fd997fd5eecb0fa7256fee3`.
- Selector report:
  `/private/tmp/bte-pmxt-first-proof-selector-2026-06-08/selector/first-proof-selector-report.json`;
  `status = selected`, `eligible_assets = 451`, `selected_asset_count = 1`,
  `selected_asset_ids_hash =
  edc5e3c70031056cf544d2cf581c5fe2ee3122886090ae513d6321a34c99d966`,
  selector-report logical content hash
  `f8315f9eaaa9207b6eaeeac604019d8aa11af0e69eb2505f1f1fa1848e72153c`, file
  SHA-256 `647f0cee89becb46b7051992dd7fea25ca08be3b551bd6873cd21be2ebd7b524`.
- Efficiency evidence: the first projected debug scan that still allocated
  strings per row took `88.30s`; after keeping parquet projection and changing
  counting to borrow per row and allocate only for new keys, the same bounded
  64.9M-row ledger scan took `55.23s` in debug. Selector report generation from
  the compact ledger took `2.27s`.
- TDD evidence: the event-count ledger API, event-count ledger CLI, and selector
  CLI each had a RED failure first, then GREEN verification through
  `scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice`.
  The full first-proof selector integration test file is the focused regression
  suite for this slice.

This closes only the generic/configured source-selector artifact gap. It does
not close PMXT raw-row-to-NT projection, `nautilus-polymarket` parser/provider
reuse inside the isolated BTE crate, selected PMXT catalog read-back, or
BacktestNode consumption of the selected source-backed catalog.

## Recommendation

Proceed with backfill-first proof, then production BTE proof; do not start
broad historical backfill or custom simulator work:

1. Keep the coverage ledger as the first gate: it now consumes manifest objects
   without venue-specific code branches, records rejected unsupported schemas
   instead of aborting, and runs before any download/conversion work.
2. Bring source-proof evidence into the current acceptance contract before
   choosing a tranche: the staged source-proof-v3 files are non-current-contract
   records under the admissibility verifier. The derivability verifier shows
   their raw payloads are S3/hash-bound, but every proof still needs explicit
   license, schema, fidelity, forbidden-claim, and NT mapping evidence before
   `SourceProofReport::accept`.
3. Use the ledger to choose one bounded accepted tranche for normalization. Do
   not choose by venue name; choose by source-binding evidence state, completed
   manifest status, object count/byte budget, data family, and gap policy.
4. Normalize that tranche into the declared table contract, preserving
   source-proof id/version, source binding, raw object hash, byte count, table
   family, instrument universe metadata, and gap reason.
5. Export an NT catalog from the normalized tranche and prove that
   `BacktestNode` consumes it using NT's catalog/data config APIs.
6. For every accepted tranche, repeat the SSM-backed `--publish-output
   --prove-published-catalog` gate under `nt-research-analytics/`; the
   reference BNBUSDC tranche now proves the path once, but broader backfill
   output must earn its own S3 listing, artifact hashes, and direct-catalog
   proof.
7. Add real typed NT mappings for leverage maps, margin model, simulation
   modules, fill model, latency model, fee model, and settlement prices only
   when accepted source proof and result-contract claim limits justify each
   surface.
8. Keep unsupported NT venue/system model surfaces rejected before NT config
   construction; the declared venue placeholders must continue producing
   structured errors until real NT mappings land.

## 2026-06-09 continuation checkpoint

The original prompt path in the active goal,
`/Users/spson/Downloads/prompts/backtesting.md`, is absent on disk. The current
matching prompt is
`/Users/spson/Downloads/New Folder With Items/prompts/backtesting.md`. Its hard
stop still applies: do not start implementation or broad backfill unless raw
source proof, clean output prefix, idempotency, NT mechanism, BacktestNode
consumption, and result-contract provenance are all known.

Current branch and CI state:

- Worktree: `codex/bte-clean-converter-nt-use-main-reconcile`.
- Current commit: `a9ce2baaa00fd56a81d9568374645ea752f02a66`.
- PR: `https://github.com/seungpyoson/bolt-v2/pull/610`.
- PR #610 is green at `a9ce2baa`: Backtester CI `bvs-test`,
  `bvs-clippy`, `bvs-fmt`, `backtester-gate`, and main CI `fmt-check`,
  `clippy`, `source-fence`, `nextest archive`, all nextest shards, `deny`,
  `check-aarch64`, `gate`, `test`, CodeQL, and actionlint passed.
- Local worktree was clean before this checkpoint update.

Open task audit against `tasks.md`:

- `BACKTESTING_ENGINE-006` remains open: Artifact Index producer IAM scope is
  still not proven end to end.
- `BACKTESTING_ENGINE-022` remains open: sample-to-NT catalog mapping is
  proven for the accepted Binance native-trades path and for the current PMXT
  one-off selected-source artifact, but broad PMXT source acceptance remains
  blocked.
- `BACKTESTING_ENGINE-039` is now repo-side complete: issue dependencies are
  linked in `spec.md` and current live issue state is captured in
  `reference/issue-dependency-status.backtesting-engine-039.2026-06-09.json`.

BTE-022 current root cause:

- The PMXT source proof
  `source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json`
  has `nt_mapping_status = accepted`, but `status = pending`,
  `source_selection_status = PENDING_MORE_PROOF`, and
  `usage_scope = one_off_backfill_data`.
- The still-pending checks are coverage, retention/freshness, completeness,
  exact accepted-window cost, and artifact-root storage proof.
- The current source-catalog mapping readiness report is correctly blocked,
  not stale: its TOML allows only `current_bte_status = accepted` and
  `parquet_catalog_status = proven`, while the PMXT mapping evaluation observes
  `one_off_current_artifact_proven_broad_backfill_blocked` for both fields.
- Therefore PMXT must remain one-off backfill evidence. It must not be promoted
  into canonical/broad source selection by widening allowed statuses or by
  treating the one-off artifact as durable acceptance.

Dynamic tick-size status:

- Pinned NT has live Polymarket tick-size-change handling and can replay
  `InstrumentStatus` and `InstrumentClose` as data streams.
- That is not timed `InstrumentAny` instrument-definition replay through
  `BacktestDataConfig`. Full PMXT L2 over tick-changing assets remains blocked
  until NT-native dynamic instrument epoch replay is proven or a separately
  accepted bounded-exclusion policy applies.

Next efficient path:

1. Do not run a broad PMXT conversion/backfill.
2. Build or refresh source-proof coverage/cost/storage evidence from indexes
   and manifests before any raw payload download.
3. Keep object/byte/time budgets in TOML, select by source-binding evidence
   state and data family, and reject any record whose source proof remains
   pending.
4. Only after a source proof is accepted should a tranche proceed into
   canonical normalization, NT catalog projection/read-back, BacktestNode run,
   and result-contract binding.
5. Old converted/catalog artifacts remain reference evidence only; do not
   delete them without a clean verified replacement and separate approval.

## 2026-06-09 source-proof-bound coverage checkpoint

Root cause addressed:

- The efficient PMXT coverage-ledger path already avoided raw payload downloads,
  but the scratch spec repeated source proof metadata by hand and omitted the
  canonical `table_family`. That left the generic binding-coverage gate unable
  to use the record for data-family-scoped readiness.
- `BackfillCoverageManifestFile` now accepts `source_proof_path`. When present,
  the coverage ledger derives `source_binding`, `table_family`,
  `source_proof_id`, `source_proof_version`, and `source_proof_status` from the
  committed `SourceProofReport` instead of duplicating those values in the
  manifest spec.
- If a manifest spec still supplies explicit source-proof metadata alongside
  `source_proof_path`, the values must match the source-proof report. Conflicts
  fail before ledger output is written.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_coverage coverage_ledger_binds_source_proof_metadata_from_report_path`
  failed because `source_proof_path` was an unknown `[[manifest]]` field.
- GREEN: the same focused test passed after the coverage-ledger manifest path
  loaded `SourceProofReport` metadata.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_coverage source_proof`
  passed 3 focused source-proof coverage tests, including conflict rejection
  for explicit `table_family` values that disagree with `source_proof_path`.
- Concrete PMXT rerun:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin backfill_coverage_ledger -- --spec /private/tmp/bte-pmxt-coverage-ledger-source-proof-20260609/pmxt-coverage-ledger.toml`
  wrote
  `/private/tmp/bte-pmxt-coverage-ledger-source-proof-20260609/ledger-output/backfill-coverage-ledger.json`
  with `content_hash =
  c1348e348e9b9ff4d5ac8aa3e52d004110bcf8b86fe1d7c7cba568dbedbb2a63`
  and file sha256
  `7dc20194f826eb9c05ec99c27cdb86057f775e1486d688a73cc2924659a4c2cf`.
  Both records now carry `source_binding =
  polymarket-parquet-archive-index`, `table_family =
  order_book_snapshot_deltas`, `coverage_axis = timestamp_received`, and the
  PMXT source proof id/version from the committed source-proof report.
- The rejected-ledger status is now committed in
  `reference/source-proof-pmxt-coverage-ledger-status.2026-06-09.json`: two
  records, both `canonical_ready = false`, both blocked by
  `source_proof_not_accepted`, with ledger file sha256
  `7dc20194f826eb9c05ec99c27cdb86057f775e1486d688a73cc2924659a4c2cf`.

Current conclusion:

- The change improves the systematic add-a-source/data-family path: coverage
  records can be bound to source-proof metadata without hardcoded venue names,
  path-family inference, or duplicate TOML values.
- PMXT remains rejected for broad backfill because the source proof is still
  `pending`; both records block on `source_proof_not_accepted`.
- This does not close `BACKTESTING_ENGINE-022`. Remaining blockers are still
  durable source-proof acceptance, expanded coverage/cost/storage proof, and
  NT-native dynamic tick-size epoch replay or an accepted bounded-exclusion
  policy.

## 2026-06-09 generic hardcode guard checkpoint

Prompt requirement addressed:

- `/Users/spson/Downloads/New Folder With Items/prompts/backtesting.md`
  requires no hardcoded runtime IDs, paths, quantities, venues, instruments, or
  hashes.
- The existing sample-venue guard blocked Bybit/BNBUSDC/Korean sample values in
  production Rust, but did not encode the newer rule that Binance is also a
  sample and that PMXT/Polymarket names must remain isolated to explicit
  one-off proof or NT-Polymarket proof modules.

Change:

- `backtesting_vertical_slice_sample_venue_guard` now bans `binance`,
  `bybit`, `bnbusdc`, Korean venue sample names, and `public_archive` from all
  production Rust.
- `pmxt` and `polymarket` are allowed only in the explicit PMXT one-off
  projection, Polymarket metadata/surface proof modules, their CLI wrappers, and
  `lib.rs` module declarations. Generic backfill/readiness/catalog/run code may
  not gain PMXT or Polymarket-specific branches.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_sample_venue_guard production_rust_does_not_hardcode_sample_venue_or_instrument`
  failed after adding `binance`, `pmxt`, and `polymarket` to the guard because
  PMXT/Polymarket appear in the intended one-off/proof modules.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_sample_venue_guard`
  passed 2 tests after adding the explicit PMXT/Polymarket proof-module
  allowlist.

Current conclusion:

- Bybit, Binance, and BNBUSDC are enforced as sample/config/reference values,
  not generic production-code constants.
- PMXT remains a one-off backfill proof path, not a reusable canonical source
  abstraction.
- This guard does not close `BACKTESTING_ENGINE-022`; it prevents future drift
  while source-proof acceptance, coverage/cost/storage, and dynamic tick-size
  replay remain unresolved.

## 2026-06-09 PMXT row-to-NT evidence reconciliation

Root cause addressed:

- `reference/source-proof-pmxt-polymarket-row-to-nt-contract.2026-06-08.json`
  still described the PMXT row-to-NT contract as
  `draft_contract_not_implemented`, but current code and later proof artifacts
  have moved the bounded one-off path past that state.
- The stale label made it look as if BTE had not yet reused NT Polymarket
  parser/catalog/backtest surfaces for the selected PMXT sample, even though
  the current branch has one-off projection, NT catalog read-back,
  `BacktestNode` execution, and result-contract binding.

Current reconciliation:

- The evidence artifact now records
  `bounded_one_off_projection_implemented_broad_backfill_blocked`.
- The bounded PMXT path remains scoped to
  `usage_scope = one_off_backfill_data`; it is not canonical source acceptance.
- Current code evidence is the explicit PMXT one-off module:
  `crates/backtesting-vertical-slice/src/pmxt_one_off_backfill_projection.rs`.
  It validates the selected-source report chain, rejects non-one-off usage
  scope, uses pinned NT `parse_gamma_market`/`create_instrument_from_def`,
  `parse_book_snapshot`, and `parse_book_deltas`, mirrors the private NT
  historical trade-id shape with provenance, writes NT `ParquetDataCatalog`
  data, and runs NT `BacktestNode` against the verified catalog.
- Current tests proving the claim are in
  `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_pmxt_one_off_projection.rs`:
  parser reuse, canonical-scope rejection, selected-source projection without
  a full source rescan, TradeTick projection/read-back, `tick_size_change`
  fail-closed behavior, mixed OrderBookDelta/TradeTick catalog metadata, and
  artifact-root result-contract binding.

Current conclusion:

- This removes a stale report contradiction. It does not close
  `BACKTESTING_ENGINE-022`.
- PMXT broad backfill remains blocked by pending source-proof acceptance,
  expanded coverage/cost/storage evidence, durable source selection, and
  NT-native dynamic tick-size epoch replay.
- The efficient path is still to keep PMXT as a one-off evidence source while
  durable Polymarket source selection and dynamic tick-size replay are proven
  separately.

## 2026-06-09 Artifact Index IAM provisioning-plan checkpoint

Root cause addressed:

- `BACKTESTING_ENGINE-006` already had direct S3 commit mechanics proof, but
  the IAM-scope proof used the broad generic SSM binding
  `/bolt/artifact-store/s3/*`.
- That proof showed denied-kind writes were not rejected:
  three denied writes were attempted, zero were rejected, and three violations
  were recorded.
- The repo had a per-kind policy generator, but no typed contract binding the
  producer kind, SSM parameter namespace, generated policy, denied proof kinds,
  and expected denied-write count into one reviewable provisioning shape.

Change:

- `artifact_index_producer_iam_provisioning_plan` now builds a deterministic
  per-kind plan from `ArtifactKind`, `artifact_root`, optional proof roots, an
  SSM parameter prefix, and explicit `denied_artifact_kinds`.
- The plan records only SSM parameter paths, never credential values.
- The plan rejects relative or wildcard SSM prefixes and rejects a proof where
  the producer kind is listed as its own denied kind.

Current conclusion:

- This makes the repo-side IAM/proof contract machine-checkable before any AWS
  mutation.
- It does not prove AWS enforcement and does not close
  `BACKTESTING_ENGINE-006`.
- The task can close only after an approved per-kind producer identity and SSM
  parameter set are provisioned, and a real `artifact_index_commit_proof` run
  shows denied-kind event, snapshot, and latest-pointer writes rejected by
  permissions.

## 2026-06-09 Artifact Index SSM readiness checkpoint

Read-only external state:

- `aws ssm describe-parameters --parameter-filters Key=Name,Option=BeginsWith,Values=/bolt/artifact-index --query Parameters[].Name --output text`
  returned no parameter names.
- `aws ssm describe-parameters --parameter-filters Key=Name,Option=BeginsWith,Values=/bolt/artifact-store --query Parameters[].Name --output text`
  returned only `/bolt/artifact-store/s3/access-key-id` and
  `/bolt/artifact-store/s3/secret-access-key`.
- No SSM parameter values were read, and no AWS mutation was performed.

Current conclusion:

- The current environment still has only the broad generic artifact-store S3
  credential binding.
- There is no existing per-kind `/bolt/artifact-index/...` producer credential
  namespace to use for a denied-kind IAM proof.
- `BACKTESTING_ENGINE-006` remains open. It can close only after explicit
  approval for AWS security mutation and provisioning of per-kind producer
  credentials, or after an approved commit coordinator/table format replaces
  the direct S3 pointer-commit path.

## 2026-06-09 coverage-ledger source-proof acceptance checkpoint

Root cause addressed:

- `SourceProofReport::evaluate_acceptance()` already rejects invalid accepted
  proofs, including one-off backfill data that cannot become canonical source
  input.
- `backfill_coverage` imported metadata from `source_proof_path` by parsing the
  JSON and copying `status`, `source_binding`, `table_family`, proof id, and
  proof version. It did not re-run accepted-proof validation when the file
  claimed `status = accepted`.
- That left the downstream coverage/readiness path weaker than the
  source-proof gate: a hand-edited accepted-looking source proof could reach
  coverage output even when the canonical acceptance validator would reject it.

Change:

- `backfill_coverage` now calls `SourceProofReport::evaluate_acceptance()` for
  any `source_proof_path` whose parsed status is `accepted`.
- If validation fails, coverage artifact writing fails with
  `SourceProofAcceptanceRejected` before a ledger record can become accepted or
  canonical-ready.
- Pending source proofs still bind source-proof metadata and produce rejected
  coverage records, preserving the efficient no-download PMXT evidence path.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_coverage coverage_ledger_rejects_source_proof_path_with_invalid_accepted_proof -- --nocapture`
  failed because the invalid accepted proof wrote
  `backfill-coverage-ledger.json`.
- GREEN: the same focused test passed after coverage revalidated accepted
  source-proof files.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_coverage source_proof -- --nocapture`
  passed 4 focused source-proof coverage tests.

Current conclusion:

- This closes a concrete slow-backfill guardrail gap without adding venue,
  source, PMXT, or table-family constants to generic production code.
- It does not close `BACKTESTING_ENGINE-022`; PMXT broad backfill still needs
  durable accepted source proof, coverage/cost/storage evidence, and dynamic
  tick-size replay proof or an accepted bounded-exclusion policy.

## 2026-06-09 BTE issue-dependency refresh checkpoint

Root cause addressed:

- `BACKTESTING_ENGINE-039` remained open because
  `1-backtesting-engine/spec.md` listed dependency issue numbers but did not
  bind them to current live issue state or a review boundary.
- GitHub issue mutation is explicitly out of scope without user approval, so
  the safe deliverable is a repo-side dependency refresh, not issue comments.

Change:

- Added
  `reference/issue-dependency-status.backtesting-engine-039.2026-06-09.json`.
- The record captures current live GitHub state for #19, #23, #24, #34, #112,
  #115, #127, #148, #158, #236, #254, and #407.
- The BTE spec now links each issue directly and states the exact BTE relation
  and non-closure boundary.
- `BACKTESTING_ENGINE-039` is marked complete for repo-side implementation
  review preparation. No GitHub issue bodies or comments were mutated.

Verification:

- GitHub connector read-only fetches confirmed all twelve listed dependency
  issues are currently open.
- The local dependency status record has
  `github_mutation_performed = false` and
  `bte_039_repo_side_dependency_audit_complete = true`.

Current conclusion:

- BTE implementation review now has a current dependency map and explicit
  non-closure boundaries.
- This does not alter the remaining engine blockers:
  `BACKTESTING_ENGINE-006` still needs approved real IAM enforcement proof, and
  `BACKTESTING_ENGINE-022` still needs durable accepted source proof plus
  broad backfill safety evidence before canonical PMXT/Polymarket use.

## 2026-06-09 L2 source-proof claim-limit checkpoint

Root cause addressed:

- `SourceProofReport::evaluate_acceptance()` validates structured
  `claim_limits`, but the previous implementation returned early for
  `L2_REPLAY` before checking that every `forbidden_claims` entry had a
  matching structured claim-limit row.
- That made an accepted L2 proof capable of carrying critical limits, such as
  "no dynamic instrument-epoch replay claim", only as prose. Result-contract
  consumers could then miss the structured provenance that explains why a
  bounded replay must not be interpreted as broad execution-quality evidence.

Change:

- `validate_claim_limits` now requires structured `claim_limits` whenever
  `forbidden_claims` are present, including `L2_REPLAY` proofs.
- Full L2 proofs with no forbidden claims can still omit claim limits.
- Non-L2 proofs still require forbidden claims and structured claim limits, so
  weaker data cannot silently carry execution-quality claims.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --lib l2_replay_forbidden_claims_require_structured_claim_limits -- --nocapture`
  failed because an L2 proof with an unstructured forbidden claim was accepted.
- GREEN: the same focused test passed after claim-limit validation applied to
  L2 forbidden claims.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --lib source_proof::tests:: -- --nocapture`
  passed 76 source-proof unit tests.

Current conclusion:

- This closes a generic BTE-022 guardrail gap without adding any
  venue/source/data-family constants.
- It does not close `BACKTESTING_ENGINE-022`: durable accepted source proof,
  coverage/cost/storage evidence, and dynamic tick-size replay proof or an
  accepted bounded-exclusion policy are still required before broad
  PMXT/Polymarket backfill can become canonical.

## 2026-06-09 source-catalog mapping proof-binding checkpoint

Root cause addressed:

- `BackfillExecutionReadiness` already bound source-selection readiness to the
  accepted tranche's `source_proof_id` and `source_proof_version`.
- `SourceCatalogMappingReadiness` only carried `source_binding`,
  `table_family`, and NT data types. A stale or hand-edited mapping-readiness
  report for the same binding/table family could therefore satisfy execution
  readiness for a different source-proof version.
- That was a generic broad-backfill safety gap: a PMXT one-off mapping artifact
  or any older provider mapping proof must not authorize a later canonical
  source proof unless the mapping evidence is for the same proof version.

Change:

- Source-catalog mapping readiness specs and reports now carry
  `source_proof_id` and `source_proof_version`.
- Mapping evaluation rows can carry the same proof identity, and readiness
  blocks with `source_proof_mismatch` when the observed mapping evidence is not
  for the requested proof.
- Backfill execution readiness now rejects source-catalog mapping readiness
  whose proof identity differs from the accepted tranche or execution plan.
- The committed Binance ready mapping report and PMXT blocked mapping report
  were refreshed so both are bound to exact source-proof versions.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_execution_readiness execution_readiness_blocks_when_source_catalog_mapping_readiness_source_proof_mismatches -- --nocapture`
  failed because `SourceCatalogMappingReadinessReport` had no
  `source_proof_id`/`source_proof_version` fields and execution readiness had
  no source-proof mismatch blocker.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_source_catalog_mapping_readiness --test backtesting_vertical_slice_backfill_execution_readiness --test backtesting_vertical_slice_backfill_gate_reference_artifacts -- --nocapture`
  passed 16 focused mapping/readiness/reference-artifact tests.

Current conclusion:

- This closes a generic `BACKTESTING_ENGINE-022` bypass without adding
  venue/source/data-family constants: source-catalog mapping proof is now bound
  to the exact source proof version before execution readiness can pass.
- It does not close `BACKTESTING_ENGINE-022`. PMXT/Polymarket broad backfill
  still requires durable accepted source proof, expanded coverage/cost/storage
  evidence, and dynamic tick-size replay proof or an accepted bounded-exclusion
  policy.

## 2026-06-09 PMXT dynamic tick-size source-proof checkpoint

Root cause addressed:

- The PMXT one-off projection tests and
  `source-proof-pmxt-polymarket-tick-size-change-status.2026-06-08.json`
  already recorded that dynamic tick-size replay is not proven through the
  pinned NT `BacktestDataConfig` catalog path.
- The pending PMXT `SourceProofReport` did not carry that exact limitation as
  both a `forbidden_claims` entry and a structured `claim_limits` row.
- That left the dynamic tick-size blocker visible in surrounding status
  artifacts but weaker in the source-proof artifact that downstream gates read.

Change:

- Added the PMXT source-proof forbidden claim:
  `No dynamic tick-size replay claim until NT-native instrument-epoch replay is
  proven.`
- Added the exact matching structured claim-limit row backed by
  `source-proof-pmxt-polymarket-tick-size-change-status.2026-06-08.json`.
- Added a reference-fixture regression test that every one-off L2 source proof
  must carry this dynamic tick-size replay exclusion as both prose and
  structured claim-limit data.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_source_proof_reference_fixtures reference_fixtures_include_unselected_binary_option_source_proof -- --nocapture`
  failed because the PMXT one-off L2 fixture did not forbid dynamic tick-size
  replay claims.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_source_proof_reference_fixtures -- --nocapture`
  passed 3 reference-fixture tests.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --lib source_proof::tests:: -- --nocapture`
  passed 76 source-proof unit tests.

Current conclusion:

- This makes the dynamic tick-size limitation machine-visible at the
  source-proof boundary before any downstream mapping/readiness/result contract
  can overclaim it.
- It does not close `BACKTESTING_ENGINE-022`: full PMXT/Polymarket L2
  acceptance still needs either NT-native dynamic instrument-epoch replay proof
  or an accepted bounded-exclusion policy plus durable source proof and
  coverage/cost/storage evidence.

## 2026-06-09 compile-checked NT tick-size surface checkpoint

Root cause addressed:

- The tick-size status artifact had source-line evidence for NT live
  Polymarket tick-size changes and BacktestNode static catalog instrument
  loading, but the BTE proof crate only compile-checked parser/provider reuse.
- That left the dynamic instrument-epoch decision less durable than the other
  NT-use claims: future dependency changes could alter public surfaces without
  a focused BTE test noticing the boundary.

Change:

- `polymarket_nt_surface_proof` now exposes
  `prove_polymarket_dynamic_instrument_epoch_surfaces`.
- The proof compile-checks NT live tick-size rebuild access through
  `rebuild_instrument_with_tick_size`, records `InstrumentStatus` and
  `InstrumentClose` as BacktestDataConfig auxiliary streams, and records the
  current decision as `StaticCatalogInstrumentLoadOnly`.
- The proof intentionally records
  `backtest_data_config_instrument_definition_stream_supported = false` because
  pinned NT `BacktestDataConfig` dispatches `Data` streams and the standard
  `Data` enum does not include timed `InstrumentAny` definition updates.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_polymarket_nt_surface bte_records_nt_dynamic_tick_size_backtest_surface_boundary -- --nocapture`
  failed because the dynamic instrument-epoch proof API did not exist.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_polymarket_nt_surface -- --nocapture`
  passed 2 Polymarket NT surface tests.

Current conclusion:

- We are using the NT surfaces that exist for this boundary: live
  Polymarket tick-size rebuilds, NT parser/provider surfaces, NT instrument
  storage, and BacktestDataConfig auxiliary status/close streams.
- We are not claiming a surface NT does not currently expose through
  BacktestDataConfig: timed `InstrumentAny` instrument-definition replay.
- This still does not close `BACKTESTING_ENGINE-022`; it strengthens the
  evidence for why broad PMXT/Polymarket L2 acceptance needs either a proven
  NT-native dynamic epoch path or a bounded exclusion policy.

## 2026-06-09 selected-source forbidden-event exclusion checkpoint

Root cause addressed:

- The PMXT one-off projection schema already carried
  `forbidden_ignored_event_types`, and the converter rejected attempts to
  silently ignore those event rows.
- The projection also read the selector report, but it did not require the
  selector's own `excluded_event_families` to cover those schema-forbidden
  event types before accepting the selected-source slice.
- That left a provenance gap: a tiny selected-source artifact could pass if it
  happened not to contain a tick-size-change row, without proving the selector
  excluded the event family at source selection time.

Change:

- `project_pmxt_selected_source_parquet_to_nt` now requires every configured
  `forbidden_ignored_event_type` to appear in the selector report's configured
  `excluded_event_families`.
- The guard is config-driven: production code compares schema-owned values to
  selector-owned values and does not hardcode a venue, source, or event name.
- `source-proof-pmxt-selected-source-slice.2026-06-08.json` now records this
  selected-source guardrail as explicit reference evidence.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_pmxt_one_off_projection pmxt_selected_source_projection_requires_selector_to_exclude_forbidden_event_types -- --nocapture`
  failed because a selector report with no excluded event families still let
  the PMXT projection succeed.
- GREEN: the same focused test passed after adding the selector exclusion
  coverage check.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_pmxt_one_off_projection -- --nocapture`
  passed 17 related PMXT projection tests.

Current conclusion:

- This closes another BTE-022 overclaim path at the selected-source boundary:
  the bounded one-off proof must now prove the excluded event family policy
  before conversion, not merely rely on a lucky tiny slice.
- It still does not close `BACKTESTING_ENGINE-022`. PMXT broad backfill still
  needs durable accepted source proof, expanded coverage/cost/storage evidence,
  and dynamic tick-size replay proof or an accepted bounded-exclusion policy.

## 2026-06-09 accepted TradeTick runtime recheck

Root cause addressed:

- The branch had several committed readiness artifacts for the accepted Binance
  one-object TradeTick path, but the current head still needed a fresh
  end-to-end runtime recheck after the PMXT/source-proof guard changes.
- This recheck intentionally uses the non-Artifact-Index execution path:
  `BACKTESTING_ENGINE-006` remains open, so this must not be mistaken for a
  producer-indexed or S3-published proof.

Verification:

- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin backtesting-vertical-slice -- --run-spec specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.binance-bnbusdc-2026-03-01.toml --execution-plan specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/execution-plan/backfill-execution-plan.json --object /private/tmp/bte-binance-bnbusdc-2026-03-01.zip --output-dir /private/tmp/bte-binance-accepted-run-head-31ead481`
  exited 0 on head `31ead4815f356445de16b6f06c40ebcba65c683c`.
- The accepted object hash was
  `433d32b8d828abee5e1937e01372d16f7edadc14c41fe736b0b9577541fa5e81`
  for 1,066,394 bytes.
- The run produced 71,431 canonical rows, wrote/read 71,431 NT `TradeTick`
  catalog rows, and `BacktestNode` processed 71,431 iterations from
  `2026-03-01T00:00:01.711256000Z` through
  `2026-03-01T23:59:59.584254000Z`.
- Result artifacts were local under
  `/private/tmp/bte-binance-accepted-run-head-31ead481`; publish output was
  disabled.
- Local artifact file hashes:
  result contract
  `50e3aa8710e2a3dcd0ad61484eb22b4cca8a035b4ed49edf2f5a61d9a35cc1f3`,
  conversion manifest
  `419fa73d941cdfe819dfdb051ad48b0ab614c231301aad8a61d61ddabc28e6cb`,
  catalog metadata
  `9b139d95254925253fd83f6234cd2e1abeccf9bdd35f2a1e085c08141f57219b`,
  accepted source proof
  `bf3f1e7fd977c7127e98508fc8b1f52c1f1841d4f393f04b6b19109a2ded6692`.

Current conclusion:

- The accepted one-object `binance-spot-native-trades` TradeTick/TRADE_REPLAY
  path is freshly proven at runtime head
  `31ead4815f356445de16b6f06c40ebcba65c683c`; commit
  `82c19678cc463bae708b96ceb2d80157e5a63a03` records the proof and is green
  in PR #610 `CI`, `Backtester CI`, and `actionlint`.
- This does not close `BACKTESTING_ENGINE-006`: no Artifact Index producer IAM
  enforcement was proven and no S3/Artifact Index mutation was attempted.
- This does not close the PMXT portion of `BACKTESTING_ENGINE-022`: broad
  PMXT/Polymarket L2 backfill still needs durable accepted source proof,
  expanded coverage/cost/storage evidence, and dynamic tick-size replay proof
  or an accepted bounded-exclusion policy.

## 2026-06-09 PMXT broad-backfill efficiency checkpoint

Root cause addressed:

- The previous slow PMXT backfill path treated large hourly Parquet objects as
  work payloads before source acceptance and bounded execution budgets were
  proven.
- PMXT v2's current public docs describe one hourly Parquet object per UTC
  hour, sorted by `(market, asset_id, timestamp_received)`, with fast
  predicates only for exact `market`, exact `asset_id`, and
  `timestamp_received` ranges. The current archive index shows recent hourly
  PMXT Polymarket objects in the hundreds of MB.
- The current one-off proof is already better than the old path: the event
  count ledger and selector carry `source_row_groups`, and
  `selected_source_slice` calls `with_row_groups(...)`, projecting 1 of 62
  row groups for the selected proof.
- The bounded proof path now has pre-payload budget controls at both levels:
  `first_proof_event_count_ledger` and `selected_source_slice` require
  source-Parquet byte budgets, and `backfill_execution_plan` requires
  accepted-tranche row-count, projected-row-group-count, and wall-time budgets.
  The remaining inefficiency is explicit and bounded: any full-object work is
  acceptable only inside a configured bounded proof, not as broad discovery.

NT/source split:

- Pinned NT provides the right live Polymarket and backtest surfaces:
  `PolymarketDataClient` handles WebSocket book snapshots, `price_change`,
  `last_trade_price`, `tick_size_change`, Gamma instruments, and NT
  `OrderBookDelta`/`TradeTick` emission; `ParquetDataCatalog` and
  `BacktestNode` remain the catalog/backtest path.
- Pinned NT does not provide a batch historical PMXT/Polymarket L2 archive
  fetcher, and standard `BacktestDataConfig` still does not expose timed
  `InstrumentAny` definition replay.
- Therefore the right boundary is unchanged: NT owns model/catalog/backtest
  semantics; Bolt owns source-proof gating, provenance, and raw historical L2
  adapters where NT has no source reader.

New evidence:

- Added
  `reference/source-proof-pmxt-broad-backfill-efficiency-status.2026-06-09.json`.
- Updated the BTE-022 status artifact with blocker
  `broad_backfill_efficiency_unproven`.

Current conclusion:

- Do not start broad PMXT/Polymarket payload download, full-object hashing,
  conversion, catalog projection, or BacktestNode runs.
- Broad backfill can proceed only after durable source proof acceptance,
  manifest/index-only coverage/cost/storage evidence, accepted object hashes,
  row-group or predicate metadata, explicit byte/row/time budgets, and dynamic
  tick-size replay or a source-proof-bound exclusion policy are proven. The
  accepted-tranche execution-plan budget fields are now machine-readable, but
  broad PMXT/Polymarket backfill remains blocked until the other evidence
  exists.
- This does not close `BACKTESTING_ENGINE-022`; it makes the slow-backfill
  non-repeat condition machine-readable.

## 2026-06-09 source-agnostic backfill boundary checkpoint

What is source-agnostic today:

- Generic gate/readiness modules are driven by `source_binding`,
  `table_family`, NT data classes, source-proof status, and configured allowed
  statuses. They do not branch on Binance, Bybit, PMXT, Polymarket, or a venue
  name.
- `backfill-source-bindings.v1.toml` carries concrete venue/source URI,
  extractor, evidence-state, fixture, product-family, and table-family values.
- The committed sample-venue guard rejects Bybit/Binance/BNBUSDC/sample venue
  literals in generic production Rust. PMXT/Polymarket names are allowed only in
  explicit one-off/proof modules and thin binaries.

What is not source-agnostic enough yet:

- The accepted-object registry is source/data-family shaped now:
  `SourceAdapterDefinition` declares adapter kind, table family, normalized
  schema version, and NT data type without venue constants. It has one durable
  registered adapter, `csv-native-trades-to-canonical-trades.v1`, which is
  systematic for accepted native trade CSV/GZIP/ZIP sources with TOML-owned
  column and side mappings. The operator preflight fails closed unless an
  explicit runner dispatch exists for the adapter kind; today that dispatch is
  only `CsvNativeTrades`, so it is not a general raw-source adapter runner.
- PMXT L2 projection is correctly isolated as a `one_off_backfill_data` adapter.
  It must not be promoted into a durable generic source path without accepted
  source proof, dynamic tick-size policy, and broad-backfill efficiency proof.
- The previously recorded `source_proof.rs` cross-market role-policy gap has
  been rechecked later on this branch: role requirements now come from
  `required_cross_market_component_roles` in the source-binding registry, and
  the production hardcode guard rejects the former sample source-family and
  component-role literals in generic production Rust.

New evidence:

- Added
  `reference/source-agnostic-backfill-boundary-status.2026-06-09.json`.
- Updated the BTE-022 status artifact with blocker
  `source_agnostic_adapter_contract_gap`.

Current conclusion:

- Adding another source that matches the existing native-trade CSV adapter can
  remain mostly TOML/source-proof/run-spec work.
- Adding a new raw format or NT data family requires a registered adapter
  contract and proof, not venue branches in generic readiness gates.
- This checkpoint does not close `BACKTESTING_ENGINE-022`; it prevents the
  incorrect claim that every future venue/data type is TOML-only today.

## 2026-06-09 source-proof policy hardcode audit checkpoint

Root cause addressed:

- The source-proof cross-market role validator had already been moved to
  `required_cross_market_component_roles` in the source-binding registry, but
  the generic hardcode guard did not reject the former sample source-family and
  component-role literals in production Rust.
- `source_proof.rs` still carried those concrete role names in a production
  doc comment, which weakened the no-hardcode evidence even though runtime
  enforcement was registry-driven.

Change:

- Extended
  `backtesting_vertical_slice_sample_venue_guard::production_rust_does_not_hardcode_sample_venue_or_instrument`
  to reject `kimchi`, `korean_spot`, `reference_price`, `fx_quote`, and
  `token_mapping` in generic production Rust.
- Reworded the `CrossMarketJoinComponent` production doc comment so role names,
  venues, and providers remain in TOML source bindings, source-proof evidence,
  or tests.
- Recorded
  `reference/source-proof-policy-hardcode-audit.2026-06-09.json`.

Evidence:

- Static production-region scan:
  `awk '/^#\\[cfg\\(test\\)\\]/{exit} {print}' crates/backtesting-vertical-slice/src/source_proof.rs | rg -n "kimchi|korean_spot|reference_price|fx_quote|token_mapping"`
  returns no matches.

Conclusion:

- Source-proof cross-market role policy is now config-owned and guarded against
  reintroducing the former sample source-family/role literals in generic
  production Rust.
- This still does not close `BACKTESTING_ENGINE-022`: the broad adapter/source
  acceptance and PMXT/Polymarket dynamic tick-size evidence remain open.

## 2026-06-09 PMXT Nautilus Polymarket surface-usage checkpoint

Question answered:

- Are we using the pinned NautilusTrader Polymarket/backtest surfaces instead
  of building a separate PMXT backtesting path?

Current answer:

- Yes for the bounded one-off proof path: PMXT projection uses NT Gamma parsing,
  BinaryOption construction, Polymarket websocket book snapshot/delta parsers,
  NT `TradeTick`/`OrderBookDelta` model types, `ParquetDataCatalog`, and
  `BacktestNode`.
- No for a direct PMXT archive reader: pinned NT has live Polymarket websocket
  and Data API clients/parsers, but it does not expose a batch reader for PMXT
  hourly Parquet L2 archives.
- The PMXT trade-id helper in pinned NT is `pub(crate)` and tied to Data API
  trade models, so the one-off adapter mirrors its transaction-hash plus asset
  plus sequence shape and adds PMXT-specific duplicate-observation collapse.
- Dynamic tick-size replay is still not closed: standard `BacktestDataConfig`
  replay dispatches catalog data classes, not scheduled `InstrumentAny`
  definition updates.

New evidence:

- Added
  `reference/source-proof-pmxt-nt-surface-usage-boundary.2026-06-09.json`.

Current conclusion:

- Do not build a custom PMXT backtesting engine.
- Keep using NT for model, catalog, and backtest execution.
- Keep Bolt-owned PMXT code limited to raw archive decoding, selected-source
  proof validation, provenance, duplicate historical trade observation policy,
  and claim-limit enforcement.
- This does not close `BACKTESTING_ENGINE-022`; it makes the "use all of what
  NT offers" boundary explicit and auditable.

## 2026-06-09 Artifact Index IAM closeout checkpoint

Current BTE-006 state:

- Direct S3 Artifact Index commit mechanics are proven: immutable event,
  snapshot, audit epoch, create-only latest pointer, conditional pointer update,
  read-back, and stale-update rejection are covered by the committed proof
  report.
- Producer IAM scope is not proven. The current broad artifact-store credential
  was able to write all three denied `research_analytics` paths during the
  real denied-kind probe.
- A current read-only SSM parameter-name probe still shows no
  `/bolt/artifact-index` producer credential namespace and only the broad
  `/bolt/artifact-store/s3/access-key-id` and
  `/bolt/artifact-store/s3/secret-access-key` parameters.

New evidence:

- Added
  `reference/artifact-index-producer-iam-closeout-plan.backtesting-engine-006.2026-06-09.json`.
- Updated
  `reference/artifact-index-commit-proof-status.backtesting-engine-006.2026-06-08.json`
  to point at the closeout plan.

Current conclusion:

- `BACKTESTING_ENGINE-006` can close through one of two paths:
  per-kind Artifact Index producer credentials, or an approved commit
  coordinator/table format.
- The concrete per-kind IAM path is: provision
  `/bolt/artifact-index/producers/backtests/access-key-id` and
  `/bolt/artifact-index/producers/backtests/secret-access-key` with a policy
  limited to `artifact-index/v1/events|snapshots|pointers/kind=backtests` plus
  audit epochs, then rerun the denied-kind proof and require three denied
  write attempts to be rejected.
- No AWS mutation was performed for this checkpoint.
- This does not close `BACKTESTING_ENGINE-006`; it removes ambiguity about the
  remaining security decision and prevents another ineffective rerun with the
  broad artifact-store credential.

## 2026-06-09 source-catalog mapping usage-scope checkpoint

Root cause addressed:

- Source-catalog mapping readiness already checked source proof id/version, NT
  data-class evidence, current BTE status, and Parquet catalog status.
- The canonical gate still relied on status strings to keep bounded
  `one_off_backfill_data` evidence separate from canonical backfill input.
- A stale or hand-edited mapping row could therefore look accepted/proven while
  carrying the wrong source-proof usage boundary.

Change:

- `SourceCatalogMappingStatusEntry` now carries optional structured
  `usage_scope`.
- `SourceCatalogMappingReadinessSpec` and reports now carry configured
  `allowed_usage_scopes`; canonical specs allow only
  `canonical_backfill_input`.
- Readiness blocks on missing usage scope or disallowed usage scope before a
  mapping report can become ready.
- The committed Binance mapping row records `canonical_backfill_input`; the
  PMXT/Polymarket mapping row records `one_off_backfill_data`, and its
  canonical readiness report is blocked with `usage_scope_not_allowed`.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_source_catalog_mapping_readiness source_catalog_mapping_readiness_blocks_one_off_usage_scope_for_canonical_gate -- --nocapture`
  failed because mapping readiness had no usage-scope field, allowed-scope
  config, observed-scope report field, or blocker.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_source_catalog_mapping_readiness source_catalog_mapping_readiness_blocks_one_off_usage_scope_for_canonical_gate -- --nocapture`
  passed after adding the structured scope gate.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_source_catalog_mapping_readiness --test backtesting_vertical_slice_backfill_execution_readiness --test backtesting_vertical_slice_backfill_gate_reference_artifacts -- --nocapture`
  passed 19 focused mapping/readiness/reference tests.

Current conclusion:

- This closes a generic `BACKTESTING_ENGINE-022` bypass without adding
  venue/source/data-family branches.
- It does not close `BACKTESTING_ENGINE-022`: durable accepted PMXT/Polymarket
  source proof, broad coverage/cost/storage evidence, and dynamic tick-size
  replay proof or an accepted bounded-exclusion policy are still required
  before broad PMXT backfill can become canonical.

## 2026-06-09 execution-readiness mapping usage-scope checkpoint

Root cause addressed:

- Source-catalog mapping readiness now records allowed and observed
  source-proof usage scope.
- The final execution-readiness gate still trusted a mapping report's
  `ready` status plus proof/binding/table/data-type fields.
- A stale or hand-edited mapping-readiness report could therefore carry a
  non-canonical observed scope while still being accepted by the final
  execution-readiness evaluator.

Change:

- `BackfillExecutionReadinessSpec` and reports now carry
  `required_source_usage_scope`.
- Execution readiness compares the source-catalog mapping readiness report's
  configured allowed scopes and observed scope against the required scope.
- A mismatch blocks with
  `source_catalog_mapping_readiness_usage_scope_mismatch`.
- The committed Binance execution-readiness specs and reports now record
  `canonical_backfill_input`.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_execution_readiness execution_readiness_blocks_when_source_catalog_mapping_readiness_usage_scope_mismatches -- --nocapture`
  failed because execution readiness had no required source-usage-scope input
  or mismatch blocker.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_execution_readiness execution_readiness_blocks_when_source_catalog_mapping_readiness_usage_scope_mismatches -- --nocapture`
  passed after adding the final gate check.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_source_catalog_mapping_readiness --test backtesting_vertical_slice_backfill_execution_readiness --test backtesting_vertical_slice_backfill_gate_reference_artifacts -- --nocapture`
  passed 20 focused mapping/execution/reference tests.

Current conclusion:

- This closes another generic `BACKTESTING_ENGINE-022` bypass without adding
  source-specific execution branches.
- It still does not close `BACKTESTING_ENGINE-022`: PMXT/Polymarket broad
  backfill remains blocked by source acceptance, broad safety evidence, and
  dynamic tick-size replay or accepted bounded-exclusion proof.

## 2026-06-09 execution-readiness mapping proof-boolean checkpoint

Root cause addressed:

- Execution readiness checked source-catalog mapping readiness status, empty
  blockers, source proof id/version, source binding, table family, NT data type,
  and source usage scope.
- It did not independently check `nt_catalog_mapping_proven`.
- A stale or hand-edited mapping-readiness report could therefore carry
  `status = ready` and no blockers while setting the explicit proof boolean to
  false.

Change:

- Execution readiness now blocks with
  `source_catalog_mapping_readiness_not_proven` when a required mapping report
  has `nt_catalog_mapping_proven = false`.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_execution_readiness execution_readiness_blocks_when_source_catalog_mapping_readiness_proof_boolean_is_false -- --nocapture`
  failed because the final execution-readiness blocker did not exist.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_execution_readiness execution_readiness_blocks_when_source_catalog_mapping_readiness_proof_boolean_is_false -- --nocapture`
  passed after adding the final proof-boolean check.

Current conclusion:

- This closes another generic `BACKTESTING_ENGINE-022` final-gate bypass.
- It still does not close `BACKTESTING_ENGINE-022`; broad PMXT/Polymarket
  backfill remains blocked by the same source-acceptance, broad evidence, and
  tick-size policy requirements.

## 2026-06-09 execution-readiness source-selection proof checkpoint

Root cause addressed:

- When `source_selection_readiness_required = true`, execution readiness checked
  source-selection readiness status, blockers, source proof id/version, source
  binding, and table family.
- It did not independently check the source-selection report's explicit proof
  booleans.
- A stale or hand-edited source-selection report could therefore carry
  `status = ready` and no blockers while one of the durable proof flags was
  false.

Change:

- Execution readiness now blocks with
  `source_selection_readiness_not_proven` when a required source-selection
  readiness report has any required proof boolean false, an acceptance error,
  or unmet required checks.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_execution_readiness execution_readiness_blocks_when_source_selection_readiness_proof_boolean_is_false -- --nocapture`
  failed because the final execution-readiness blocker did not exist.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_execution_readiness execution_readiness_blocks_when_source_selection_readiness_proof_boolean_is_false -- --nocapture`
  passed after adding the final source-selection proof check.

Current conclusion:

- This closes a generic final-gate bypass for runs that require
  source-selection readiness.
- It does not close `BACKTESTING_ENGINE-022` or authorize broad
  PMXT/Polymarket backfill.

## 2026-06-09 execution-readiness source-selection usage-scope checkpoint

Root cause addressed:

- Execution readiness now checks source-selection proof booleans when a
  source-selection readiness report is required.
- It still did not independently compare the source-selection report's
  `usage_scope` field to the configured execution scope.
- A stale or hand-edited source-selection readiness report could therefore
  retain proof booleans while carrying `one_off_backfill_data`.

Change:

- Execution readiness now blocks with
  `source_selection_readiness_usage_scope_mismatch` when the required
  source-selection readiness report's `usage_scope` differs from
  `required_source_usage_scope`.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_execution_readiness execution_readiness_blocks_when_source_selection_readiness_usage_scope_mismatches -- --nocapture`
  failed because the final execution-readiness blocker did not exist.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_execution_readiness execution_readiness_blocks_when_source_selection_readiness_usage_scope_mismatches -- --nocapture`
  passed after adding the final source-selection usage-scope check.

Current conclusion:

- This closes another generic final-gate bypass for runs that require
  source-selection readiness.
- It does not close `BACKTESTING_ENGINE-022` or authorize broad
  PMXT/Polymarket backfill.

## 2026-06-09 Artifact Index proof root-binding checkpoint

Root cause addressed:

- Execution readiness required Artifact Index commit proof mechanics,
  producer IAM scope, and artifact kind.
- It did not verify that the proof's `artifact_root` belonged to the same
  configured artifact root as the execution plan's `output_prefix`.
- A stale proof from a different configured artifact root could therefore
  authorize an indexed execution plan for the wrong root if all other proof
  booleans matched.

Change:

- Execution readiness now blocks with
  `artifact_index_commit_proof_artifact_root_mismatch` when an Artifact Index
  proof root is neither the plan's configured artifact root nor an
  `artifact-index/proofs` sandbox under that same root.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_execution_readiness execution_readiness_blocks_index_backfill_when_artifact_index_root_mismatches_plan_output -- --nocapture`
  failed because the final execution-readiness blocker did not exist.
- GREEN:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_execution_readiness execution_readiness_blocks_index_backfill_when_artifact_index_root_mismatches_plan_output -- --nocapture`
  passed after adding the root-boundary check.
- RED/GREEN refinement:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_execution_readiness execution_readiness_accepts_artifact_index_proof_sandbox_under_plan_artifact_root -- --nocapture`
  first failed because same-root proof sandboxes were too strictly rejected,
  then passed after deriving the configured root from the plan output prefix.

Current external BTE-006 audit:

- A fresh read-only SSM metadata probe at `2026-06-09T09:51:57Z` returned no
  parameter names under `/bolt/artifact-index`.
- No credential values were read and no AWS mutation was performed.
- The observation is recorded in
  `reference/artifact-index-producer-iam-closeout-plan.backtesting-engine-006.2026-06-09.json`.

Current conclusion:

- This closes a generic final-gate bypass for indexed execution readiness.
- It does not close `BACKTESTING_ENGINE-006`: per-kind producer IAM scope or an
  approved coordinator/table format is still required before relying on
  Artifact Index producer commits for broad backfill.

## 2026-06-09 binding-coverage canonical acceptance checkpoint

Root cause addressed:

- Combined backfill readiness already required the selected binding coverage to
  have both canonical-ready and accepted ledger records.
- The lower-level binding coverage report could still return `ready` for a
  configured required binding when ledger records existed but all were rejected
  or non-canonical.
- That left one gate layer too permissive for manifest-only or pending-source
  coverage evidence.

Change:

- Binding coverage now blocks with
  `required_binding_without_canonical_ready_coverage` when a required configured
  binding has ledger records but none are canonical-ready.
- It also blocks with `required_binding_without_accepted_coverage` when a
  required configured binding has ledger records but none are accepted.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_binding_coverage binding_coverage_blocks_required_records_without_canonical_ready_acceptance -- --nocapture`
  failed because the binding-coverage blockers did not exist.
- GREEN:
  same command passed after adding the fail-closed lower-level coverage checks.

Current conclusion:

- This closes a generic coverage-readiness bypass for required configured
  bindings.
- It does not close `BACKTESTING_ENGINE-022`: broad PMXT/Polymarket backfill
  still requires durable accepted source proof, broad coverage/cost/storage
  evidence, bounded budgets, and NT-compatible dynamic tick-size policy.

## 2026-06-09 current-head bounded native-trades conversion rerun

Scope:

- Rechecked only the accepted Binance BNBUSDC native-trades materialized run
  spec on post-guard head `3e38528369372e75399d01386ab7b5f74f9c9f82`
  (`Fail closed on unaccepted binding coverage`).
- The run wrote only to the clean scratch root
  `/private/tmp/bte-binance-materialized-run-current-pYrehuc1`.
- `--publish-output` was not used; no production artifact publication or
  deletion was performed.

Command:

```text
python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin backtesting-vertical-slice -- --run-spec /Users/spson/Projects/Claude/bolt-v2/.worktrees/bte-clean-converter-nt-use-main-reconcile/specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/materialized-run-spec/backfill-run-spec.toml --execution-plan /Users/spson/Projects/Claude/bolt-v2/.worktrees/bte-clean-converter-nt-use-main-reconcile/specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/execution-plan/backfill-execution-plan.json --object /private/tmp/bte-binance-bnbusdc-2026-03-01.zip --output-dir /private/tmp/bte-binance-materialized-run-current-pYrehuc1
```

Observed evidence:

- Run-spec SHA256:
  `498fcdea74e5089d722e26913e77a53d49bb6245a3234f157107184100782bfe`.
- Execution-plan SHA256:
  `a27e8a9094a990e700eb63a700207a46bffa3452426b6203c4de1d079b995855`.
- Raw accepted object SHA256:
  `433d32b8d828abee5e1937e01372d16f7edadc14c41fe736b0b9577541fa5e81`.
- `canonical_trades_rows = 71431`,
  `catalog_read_back_trade_ticks = 71431`, and NT BacktestNode iterations
  `71431`.
- Catalog hash:
  `8c128fe5acbb2e0df7c0f9b30d80de16acb285ca95f67a7bfc08c969f6b48362`.
- Result contract file SHA256 after the rerun:
  `c523e5b27266e52cc1f05f79674a3a016c933fae5897970c224987d4c11e05f7`.
- Catalog metadata file SHA256:
  `dadd665097a494e3ea8301bc3f343b14fa7213466557e5d3ea925329bd0f4aef`.
- Conversion manifest file SHA256:
  `419fa73d941cdfe819dfdb051ad48b0ab614c231301aad8a61d61ddabc28e6cb`.
- Conversion checkpoint file SHA256:
  `7e203ac5d90fc8d1b48e0455a54c9c0edb40bed8e7a943818624bc1e28c28119`.
- Canonical parquet file SHA256:
  `be908f312b61d112a25748d6641a9162f12d568f80d3f2ca94180285cc821a91`.
- The pre-rerun and post-rerun result-contract file hashes were both
  `c523e5b27266e52cc1f05f79674a3a016c933fae5897970c224987d4c11e05f7`,
  proving byte-idempotent completed-output reuse for this bounded path.

Current conclusion:

- The accepted native-trades converter, NT catalog readback, BacktestNode
  consumption, and result-contract idempotence proof still hold on current
  head after the execution-readiness and coverage guard commits.
- This evidence does not close `BACKTESTING_ENGINE-006` or
  `BACKTESTING_ENGINE-022`, and it does not authorize broad PMXT/Polymarket
  backfill or production publish/delete actions.

## 2026-06-09 durable adapter dispatch guard checkpoint

Root cause addressed:

- The source adapter registry declared adapter kind, table family, normalized
  schema version, and NT data type.
- The durable accepted-object operator path already intended to support only
  `CsvNativeTrades`, but there was no regression test proving that a registered
  non-durable adapter kind without an explicit runner dispatch fails before
  artifact writes.
- That left the source-agnostic adapter boundary under-proven: future adapter
  registration drift could look like TOML-only support for a new data family.

Change:

- Added a test-only synthetic `OrderBookDelta` source adapter fixture under
  `cfg(test)`.
- Production `REGISTERED_SOURCE_ADAPTERS` still contains only the native-trade
  CSV adapter; `REGISTERED_TRADE_CONVERTERS` remains only that durable trade
  converter.
- Added an operator regression test proving a registered non-durable adapter
  kind is rejected with the durable runner-dispatch guard before
  `conversion-checkpoint.json` can be written.
- Applied the two current clippy `manual_contains` mechanical fixes in
  readiness membership checks.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked operator::tests::run_from_run_spec_rejects_registered_non_durable_adapter_before_artifacts -- --nocapture`
  failed because the test registry did not include a non-durable adapter
  fixture.
- GREEN focused:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --lib operator::tests::run_from_run_spec_rejects_registered_non_durable_adapter_before_artifacts -- --nocapture`
  passed after adding the test-only fixture.
- Registry focused:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --lib source_adapter_registry -- --nocapture`
  passed.
- Library tests:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --lib --quiet`
  passed with 251 tests on the final code state.
- Focused readiness tests:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_execution_readiness --test backtesting_vertical_slice_source_catalog_mapping_readiness`
  passed with 21 tests after the clippy membership cleanup.
- Formatting:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- fmt --check`
  passed.
- Clippy:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- clippy --locked --lib -- -D warnings`
  passed.

Current conclusion:

- This strengthens the BTE-022 source-agnostic adapter boundary: registering an
  adapter metadata row is not enough to make the durable operator run it.
- It does not close `BACKTESTING_ENGINE-022`. New non-trade-CSV raw formats or
  NT data families still require a real adapter implementation, source-proof
  acceptance, coverage/cost/storage evidence, and NT catalog/BacktestNode proof.

## 2026-06-09 current-head PMXT selected-source conversion rerun

Scope:

- Rechecked only the bounded PMXT selected-source one-off path on current head
  `4ea54d806183fb82371de2088034de03c737b633`.
- The run used the already bounded selected-source artifact at
  `/private/tmp/bte-pmxt-current-rowgroup-proof-2026-06-08/selected-source/selected-source.parquet`.
- The run wrote only to the clean scratch root
  `/private/tmp/bte-pmxt-current-head-rerun-2026-06-09`.
- No production artifact publication, deletion, or broad PMXT payload work was
  performed.

Root-cause check:

- Rerunning the older current-schema scratch prefix
  `/private/tmp/bte-pmxt-current-schema-rerun-2026-06-09` failed because the
  existing result contract differed from newly generated stable current-head
  content.
- That prefix is now historical only. The create-only result-contract guard did
  the correct thing by refusing to overwrite it.

Command:

```text
python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin pmxt_one_off_l2_artifact_root_run -- --spec /private/tmp/bte-pmxt-current-head-rerun-2026-06-09/artifact-root-run.toml
```

Observed evidence:

- Result contract hash:
  `6c2b71bca7e5e40800aa72c0b9b2395b3d1ed08dce052349c4662d8cbc9a46de`.
- Conversion manifest logical hash:
  `a2b5407aa56068bb31e094e2d28dec71c8c2a87d592c6e1b3f30803a2ddc3c7f`.
- Catalog hash:
  `3a26bebf03e4a2c4eef1bd344a8b1c6f1b78ef7d3c7f43d6279ac9d029fab236`.
- Selected-source parquet hash:
  `0102068effdcdbb308d9390746afa6a75dfda1b3ba8fc3239ecdb4c74d9ae99e`.
- Event-count ledger hash:
  `985808244f540656dc5021703f2a2d9ae9a93305ebb5afe0b05f45a58027f00a`.
- Selected asset ids hash:
  `1e6a537007d5fb693057a9e7a51704411366c5add19d59e586d098516ff5a110`.
- The run projected `5` selected L2 source rows into an NT catalog containing
  `103` `OrderBookDelta` rows and `1` `TradeTick` row.
- NT BacktestNode consumed `104` iterations.
- Repeating the same clean-prefix command preserved result contract hash
  `6c2b71bca7e5e40800aa72c0b9b2395b3d1ed08dce052349c4662d8cbc9a46de`
  and `nt_iterations = 104`.

Current conclusion:

- The bounded PMXT selected-source conversion, NT catalog write/read path, and
  BacktestNode consumption still reproduce on current head when run against a
  clean current-schema output prefix.
- This evidence supersedes the older `/private/tmp/bte-pmxt-current-schema-rerun-2026-06-09`
  prefix as current evidence.
- This does not close `BACKTESTING_ENGINE-022`: PMXT is still one-off only,
  durable Polymarket source proof remains unaccepted, dynamic tick-size replay
  remains unproven, and broad PMXT coverage/cost/storage evidence remains
  unaccepted.

## 2026-06-09 object selection metadata gate checkpoint

Scope:

- Addressed only the generic broad-backfill efficiency guard for carrying
  object-level selection metadata through pre-payload gates.
- No PMXT source proof was accepted, no PMXT payload was downloaded, and no
  broad conversion/backtest was started.

Root-cause check:

- The prior broad-backfill status required object-level row-group or predicate
  metadata before broad payload work, but the source-proof-scope,
  accepted-tranche, and execution-plan structures could not yet carry that
  metadata.
- Without a required-metadata gate, a future execution plan could be marked
  ready for a broad source even though selected-source projection would have to
  rediscover row groups by payload scanning.

Change:

- `BackfillSourceProofScopeObject` now preserves optional `source_row_groups`
  or `row_groups` arrays and optional `predicate_ref` from manifest payload
  records.
- `BackfillAcceptedTrancheObject` and `BackfillExecutionPlanObject` now carry
  `source_row_groups` and `predicate_ref` forward.
- `BackfillExecutionPlanSpec` and `BackfillExecutionWorkBudget` now expose
  `require_object_selection_metadata`; when true, execution planning blocks
  with `ExecutionPlanObjectSelectionMetadataMissing` before payload fetch if an
  object has neither row-group nor predicate metadata.
- The flag defaults false, preserving existing accepted Binance/native
  reference gates.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_source_proof_scope --test backtesting_vertical_slice_backfill_accepted_tranche --test backtesting_vertical_slice_backfill_execution_plan -- --nocapture`
  failed because the metadata fields and required-metadata issue did not exist.
- GREEN focused:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_source_proof_scope --test backtesting_vertical_slice_backfill_accepted_tranche --test backtesting_vertical_slice_backfill_execution_plan --test backtesting_vertical_slice_backfill_run_spec_materialization --test backtesting_vertical_slice_backfill_execution_readiness --test backtesting_vertical_slice_backfill_gate_reference_artifacts`
  passed 35 tests after metadata propagation and the opt-in fail-closed guard.
- Formatting:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- fmt --check`
  passed.
- Clippy:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- clippy --locked --lib -- -D warnings`
  passed.
- Library tests:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --lib --quiet`
  passed 251 tests.

Current conclusion:

- This narrows one BTE-022 broad-backfill efficiency gap at the generic gate
  layer: accepted object-selection metadata can now flow into the execution
  plan and can be required before payload-scale work.
- It does not close `BACKTESTING_ENGINE-022`. Broad PMXT still needs accepted
  durable source proof, manifest/index-only coverage and cost, accepted object
  hashes, actual PMXT row-group or predicate metadata, bounded execution
  budgets, and dynamic tick-size replay or bounded-exclusion proof.

## 2026-06-09 Artifact Index producer SSM prefix guard checkpoint

Scope:

- Addressed only a local BTE-006 provisioning-plan guard.
- No AWS IAM users, policies, access keys, SSM parameters, S3 objects, or
  Artifact Index pointers were created or mutated.

Root-cause check:

- The BTE-006 closeout plan requires per-kind Artifact Index producer
  credentials or an approved coordinator/table format.
- The local IAM provisioning-plan generator already produced the preferred
  `/artifact-index/producers/<kind>/...` shape, but its SSM prefix validator did
  not reject a broad artifact-store prefix before generating credential paths.

Change:

- `validate_ssm_parameter_prefix` in `artifact_index_iam_policy.rs` now requires
  prefixes to end with `/artifact-index/producers`.
- The guard still allows environment-specific leading path components, but
  rejects broad prefixes such as `/example/artifact-store/s3`.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_artifact_index_iam_policy -- --nocapture`
  failed because `/example/artifact-store/s3` generated
  `/example/artifact-store/s3/backtests/access-key-id` and
  `/example/artifact-store/s3/backtests/secret-access-key`.
- GREEN focused:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_artifact_index_iam_policy -- --nocapture`
  passed after the prefix guard.
- Reference artifact focused:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_artifact_index_iam_policy --test backtesting_vertical_slice_backfill_gate_reference_artifacts`
  passed 7 tests.
- Formatting:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- fmt --check`
  passed.
- Clippy:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- clippy --locked --lib -- -D warnings`
  passed.

Current conclusion:

- This reduces the chance of regenerating a BTE-006 closeout plan against the
  current broad artifact-store credential namespace.
- It does not close `BACKTESTING_ENGINE-006`: AWS security mutation or an
  approved coordinator/table design is still required before producer IAM scope
  can be proven and before broad backfill can rely on Artifact Index producer
  commits.

## 2026-06-09 source usage scope pre-payload binding checkpoint

Scope:

- Addressed only the generic pre-payload provenance path for
  `source_usage_scope`.
- No PMXT source proof was accepted, no PMXT payload was downloaded, and no
  broad conversion/backtest was started.

Root-cause check:

- Source proof acceptance already rejects `one_off_backfill_data`, but the
  downstream source-proof-scope report, accepted tranche, materialized run spec,
  and execution plan did not all explicitly persist the usage scope.
- Without that binding, a future run spec could rely on parser defaults or drift
  from the accepted tranche before payload fetch.

Change:

- `BackfillSourceProofScopeReport`, `BackfillAcceptedTrancheManifest`,
  `BackfillExecutionRunBinding`, and `BackfillExecutionPlan` now carry
  `source_usage_scope`.
- `BackfillExecutionPlanIssue` now includes
  `RunSpecSourceUsageScopeMismatch`, so execution planning blocks when the
  accepted tranche scope and materialized run-spec scope differ.
- Backfill run-spec materialization now writes the accepted tranche scope into
  `source_proof.usage_scope` in the materialized TOML.
- Canonical JSON artifacts keep their existing wire shape through
  default/skip serialization; the explicit non-canonical path remains visible
  and blocked.

Verification so far:

- RED source-scope chain:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_source_proof_scope --test backtesting_vertical_slice_backfill_accepted_tranche --test backtesting_vertical_slice_backfill_execution_plan -- --nocapture`
  failed because the usage-scope fields and
  `RunSpecSourceUsageScopeMismatch` did not exist.
- GREEN source-scope chain:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_source_proof_scope --test backtesting_vertical_slice_backfill_accepted_tranche --test backtesting_vertical_slice_backfill_execution_plan -- --nocapture`
  passed 17 tests after usage-scope propagation and mismatch blocking.
- RED materializer:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_run_spec_materialization -- --nocapture`
  failed because the materialized TOML omitted `source_proof.usage_scope`.
- GREEN materializer:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_run_spec_materialization -- --nocapture`
  passed 2 tests after materialization copied the accepted tranche scope.

Current conclusion:

- This narrows a BTE-022 overclaim path: source usage scope can now be carried
  through the pre-payload artifact chain and run-spec drift is rejected before
  payload fetch.
- It does not close `BACKTESTING_ENGINE-022`. Broad PMXT still lacks accepted
  durable canonical source proof, accepted manifest/index coverage and cost,
  actual row-group or predicate metadata, and dynamic tick-size replay or a
  bounded-exclusion proof.

## 2026-06-09 materialized run-spec usage-scope refresh checkpoint

Scope:

- Refreshed only the committed Binance BNBUSDC accepted native-trades reference
  gate after materialization began explicitly writing `source_proof.usage_scope`.
- No broad PMXT/Polymarket payload work, production publish, delete, or AWS
  mutation was performed.

Root-cause check:

- The materializer now writes `usage_scope = "canonical_backfill_input"` into
  materialized run specs, but the committed Binance reference TOML was produced
  before that field existed.
- Because the materializer is create-only/idempotent, rerunning from the
  committed spec would reject the old committed TOML as dirty unless the
  reference artifact and dependent plan hashes were refreshed.

Change:

- Added explicit `usage_scope = "canonical_backfill_input"` to the committed
  Binance materialized run spec.
- Updated the committed execution plan `run_spec_hash` to
  `90a6be6a7512339581c35292464806cdb46e61af7ec65abf41497fb4f0349455`.
- Updated both committed execution-readiness reports to bind execution-plan
  SHA256 `77ee3edf0a31a5d9b55ba84e3e21e2030bde3295a4a7e925f84e1fffa58d4bbe`.

Verification:

- RED:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_gate_reference_artifacts binance_backfill_gate_commits_materialized_run_spec_before_execution_plan -- --nocapture`
  failed because the committed materialized run spec did not explicitly bind
  canonical source usage scope.
- GREEN reference artifacts:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_backfill_gate_reference_artifacts -- --nocapture`
  passed 4 tests after the TOML, execution plan, and readiness hash refresh.
- GREEN local conversion/backtest:
  `python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- run --locked --bin backtesting-vertical-slice -- --run-spec /Users/spson/Projects/Claude/bolt-v2/.worktrees/bte-clean-converter-nt-use-main-reconcile/specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/materialized-run-spec/backfill-run-spec.toml --execution-plan /Users/spson/Projects/Claude/bolt-v2/.worktrees/bte-clean-converter-nt-use-main-reconcile/specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/execution-plan/backfill-execution-plan.json --object /private/tmp/bte-binance-bnbusdc-2026-03-01.zip --output-dir /private/tmp/bte-binance-materialized-run-scope-current-20260609`
  exited 0, then an idempotence rerun against the same output prefix also
  exited 0.

Observed evidence:

- Run-spec SHA256:
  `90a6be6a7512339581c35292464806cdb46e61af7ec65abf41497fb4f0349455`.
- Execution-plan SHA256:
  `77ee3edf0a31a5d9b55ba84e3e21e2030bde3295a4a7e925f84e1fffa58d4bbe`.
- Output root:
  `/private/tmp/bte-binance-materialized-run-scope-current-20260609`.
- Raw accepted object SHA256:
  `433d32b8d828abee5e1937e01372d16f7edadc14c41fe736b0b9577541fa5e81`.
- `canonical_trades_rows = 71431`,
  `catalog_read_back_trade_ticks = 71431`, and NT BacktestNode iterations
  `71431`.
- Catalog hash:
  `8c128fe5acbb2e0df7c0f9b30d80de16acb285ca95f67a7bfc08c969f6b48362`.
- Result contract file SHA256 after completed-output reuse:
  `d0e074e1aa033f91d4dd3691d16af81b765f764069b0feac022f081f65258a36`.
- Catalog metadata file SHA256:
  `f1a75bcc80476c29b8adc7e51cc53859f1090acec912977647c3b30930491fdf`.
- Conversion manifest file SHA256:
  `419fa73d941cdfe819dfdb051ad48b0ab614c231301aad8a61d61ddabc28e6cb`.
- Conversion checkpoint file SHA256:
  `7e203ac5d90fc8d1b48e0455a54c9c0edb40bed8e7a943818624bc1e28c28119`.
- Canonical parquet file SHA256:
  `be908f312b61d112a25748d6641a9162f12d568f80d3f2ca94180285cc821a91`.

Current conclusion:

- The accepted Binance native-trades conversion/backtest path remains current
  after explicit source-usage-scope materialization.
- This does not close `BACKTESTING_ENGINE-006` or `BACKTESTING_ENGINE-022`, and
  it does not authorize broad PMXT/Polymarket backfill or production
  publish/delete actions.
## 2026-06-15 BTE-006 backtests producer IAM scope proof

Scope:

- Performed the approved AWS security mutation for the `backtests` Artifact
  Index producer only.
- Created IAM user `bolt-artifact-index-producer-backtests` with inline policy
  `ArtifactIndexProducerBacktests`.
- Stored the generated access key fields as SSM `SecureString` parameters at
  `/bolt/artifact-index/producers/backtests/access-key-id` and
  `/bolt/artifact-index/producers/backtests/secret-access-key`.
- No credential values were printed or committed; transient local key-material
  files were removed.

Proof:

- The repo-local Rust proof runner command was attempted through
  `scripts/rust_verification.py` and refused by policy with
  `local_compile_disabled`; no break-glass or direct cargo bypass was used.
- A focused AWS S3 API proof then used the new SSM-backed producer credentials
  to write three create-only `backtests` Artifact Index proof objects under
  the policy-approved proof root: event, snapshot, and latest pointer.
- The same credential attempted three create-only writes under
  `kind=research_analytics` for event, snapshot, and latest pointer; all three
  were rejected by AWS permissions.
- A follow-up scoped AWS S3 API proof used the same backtests producer
  credential to create a new event, snapshot, and audit epoch object, update
  the existing latest pointer with `If-Match`, reject a stale `If-Match`
  update, and reject 15 denied event/snapshot/latest-pointer writes across
  every other current Artifact Index kind: `raw`, `nt_catalog`,
  `source_proofs`, `artifact_index`, and `research_analytics`.

Committed evidence:

- `reference/artifact-index-commit-proof.backtesting-engine-006-iam-scope-backtests.2026-06-15.toml`
- `reference/artifact-index-producer-iam-scope-proof.backtesting-engine-006.backtests.2026-06-15.json`
- `reference/artifact-index-producer-iam-scope-proof.backtesting-engine-006.backtests-complete.2026-06-15.json`
- Updated
  `reference/artifact-index-commit-proof-status.backtesting-engine-006.2026-06-08.json`

Current conclusion:

- Direct S3 Artifact Index commit mechanics were already proven by the
  2026-06-08 direct S3 report.
- Backtests producer IAM scope is now proven for event, snapshot, and latest
  pointer writes against every current non-`backtests` Artifact Index kind, and
  the same scoped credential has exercised create-only event/snapshot/audit
  writes plus conditional latest-pointer update/stale-ETag rejection.
- The earlier 2026-06-08 IAM-scope report remains historical failed evidence:
  it used the generic `/bolt/artifact-store/s3/*` credential and allowed the
  denied `research_analytics` writes.
- This checkpoint does not mark broad `BACKTESTING_ENGINE-006` closed yet. The
  remaining scope decision is whether BTE-006 needs only the BTE backtests
  producer credential proven here, or whether separate current/future
  non-`backtests` producer identities must also be provisioned and proved before
  checking the task.

## 2026-06-15 BTE-006 all current producer IAM scope proof

Scope:

- Completed the broad path instead of narrowing `BACKTESTING_ENGINE-006`.
- Provisioned scoped Artifact Index producer identities and SSM `SecureString`
  credential parameters for every remaining current `ArtifactKind`: `raw`,
  `nt_catalog`, `source_proofs`, `artifact_index`, and `research_analytics`.
- Combined with the existing scoped `backtests` producer proof, every current
  Artifact Index kind now has a per-kind producer credential namespace under
  `/bolt/artifact-index/producers/<artifact_kind>/`.
- No credential values were printed or committed; transient local key-material
  files were removed.

Proof:

- For each newly provisioned non-`backtests` producer, the scoped credential
  created an event, two snapshots, an audit epoch, an initial latest pointer,
  and a conditional latest-pointer update under its proof root.
- Each proof rejected stale latest-pointer `If-Match` reuse.
- Each proof attempted event, snapshot, and latest-pointer writes against the
  other five current Artifact Index kinds; all 75 denied writes were rejected,
  with `violation_count = 0`.
- The all-producer proof is recorded in
  `reference/artifact-index-all-producer-iam-scope-proof.backtesting-engine-006.2026-06-15.json`
  with file SHA256
  `cc762ae15550340b1f13ca56cc7302dbaa279b6bdc087311fb8464d8dcbf4474`.

Current conclusion:

- Direct S3 Artifact Index commit mechanics remain proven by the 2026-06-08
  direct S3 report.
- Producer IAM scope is now proven for all six current Artifact Index kinds:
  `raw`, `nt_catalog`, `source_proofs`, `backtests`, `artifact_index`, and
  `research_analytics`.
- `BACKTESTING_ENGINE-006` is checked complete for the current `ArtifactKind`
  set. Future Artifact Index kinds or changed path shapes require their own
  scoped proof before relying on them.
