# Spec: Backtesting Engine

## Scope

Build a future thin NT-native backtesting orchestration layer. It consumes
approved catalog projections and run manifests, executes NT backtests, and emits
NT-derived results with source hashes, fidelity labels, and claim limits.

This is the default first implementation vertical for the package. Research
Analytics and Dashboard are downstream consumers unless a future session
explicitly selects those projects.

This is separate from Research Analytics and Dashboard. It does not own
notebooks, dashboard UI, provider capture, live trading, or a custom simulator.

## Users

- Researcher: runs reproducible backtests over approved catalog projections.
- Maintainer: reviews manifest-to-NT config mapping, fidelity claims, and
  extension-surface decisions before accepting results.

## Requirements

- Use NT `BacktestNode`/`BacktestEngine` before any Bolt-owned backtest behavior.
- Use NT `ParquetDataCatalog` and NT core data classes for replay/backtest input.
- Prove the resolved `bolt-v2` NT dependency can read/write/query
  `ParquetDataCatalog` data from the configured S3 `artifact_root`, including
  required crate features and storage options, before relying on S3 as the
  runtime catalog path.
- Treat the run manifest as a thin NT config map plus Bolt orchestration
  metadata, not a competing domain language.
- Keep venue/product/provider identity in TOML-selected registry/binding data.
- Name first proof fixtures by market structure, not venue: `binary option`
  and `perps/spot`.
- Treat venue/provider names in this package as candidate bindings and evidence
  examples only, never as architecture branches.
- Support at least two TOML/registry-selected binding fixtures so a single
  hardcoded path cannot pass review.
- Classify each backtesting surface as `defaulted`, `pass_through`,
  `custom_owned`, or `unsupported_for_now`.
- Materialize resolved NT/default values in the run manifest.
- Allow custom-owned components only through NT-compatible contracts that do not
  create independent execution, PnL, position, fill, account, or portfolio truth.
- Fail fast when a requested surface is `unsupported_for_now`.
- Label each result by data fidelity: `L2_REPLAY`, `TRADE_BAR_REPLAY`,
  `SIGNAL_ONLY`, or `FORWARD_CAPTURE_PENDING`.
- Evaluate the highest-fidelity historical market data available for each
  fixture before accepting weaker replay. Execution-quality claims require
  source-proven L2/L3 order-book evidence and matching NT replay proof.
- Use L2 order-book data whenever source proof, license, sample, and NT catalog
  mapping pass; otherwise cap result claims to the proven source fidelity.
- Require an accepted thin `SourceProofReport` before any source becomes
  canonical NT catalog input or backtest input. It is a Bolt-owned proof pointer
  and claim-limit gate, not an NT object or heavy data store.
- Own source-proof acceptance for catalog/backtest use; downstream consumers
  cannot mark proof accepted, upgrade fidelity, or weaken forbidden claims.
- Allow automated source-proof acceptance only when every required schema,
  sample, license, time/freshness, NT mapping, fidelity, and forbidden-claim
  check passes; ambiguous or failed proof stays pending/rejected.
- Keep accepted source-proof records immutable; changed facts create a new
  proof version that supersedes the prior accepted proof.
- Keep old backtest results valid as historical artifacts when their proof is
  superseded; new runs use latest accepted proof unless the manifest explicitly
  pins an older accepted proof for reproducibility with an allowed
  `proof_pin_reason_code`.
- Require `run_purpose`; `normal` runs cannot pin non-latest proof. Older proof
  pins are allowed only for `reproduction`, `audit`, `regression`, or
  `migration` runs.
- Treat NT version, strategy config hash, catalog hash, manifest schema, and
  execution-model currentness as separate future manifest rules, not as part of
  the source-proof pin policy.
- Defer exact currentness rules for those non-source-proof dimensions to the
  future manifest-schema work.
- Emit result records with NT pointer, catalog/source hashes, strategy config
  hash, fill model, fee model, fidelity class, and claim limits.
- Keep `BacktestResultContract` objective: it may expose metrics, report
  pointers, evidence hashes, warnings, and mechanical blockers, but it must not
  recommend whether to use or escalate a strategy.
- Treat strategy escalation decisions as Research Analytics experiment-result
  verdict or later review-artifact scope, not Backtesting Engine scope.
- Publish canonical artifacts through the cross-project Artifact Index Contract; normal
  discovery must use committed index snapshots, not recursive S3 listing.
- Treat Artifact Index writes as producer-owned: Backtesting Engine writes
  records only for artifacts it produces and must not grant consumers authority
  to mutate those records.

## Fidelity Classes

| Class | Meaning | Allowed use | Forbidden use |
|---|---|---|---|
| `L2_REPLAY` | Historical L2/L3 order-book replay supports execution-quality backtest claims after NT catalog projection proof. Acceptable evidence is source-order-preserving deltas or snapshots frequent enough for the strategy decision interval, with explicit limits for unproven queue behavior. | Execution-quality replay/backtest claims for proven venue/source/instrument scope. | Extending the claim to venues, sources, instruments, queue position, or sub-snapshot liquidity behavior without proof. |
| `TRADE_BAR_REPLAY` | Trades, fills, candles, or bars support price/alpha research but not full queue/execution simulation. | Price path, alpha, fills-history, or bar/trade replay analysis with limitations. | Queue position, order-book liquidity, or execution-quality claims. |
| `SIGNAL_ONLY` | Data can inform signals, features, provenance, or dashboards but not execution-quality backtests. | Feature generation, provenance, dashboards, exploratory research. | Backtest execution quality or simulated fill claims. |
| `FORWARD_CAPTURE_PENDING` | No sufficient history exists; capture can start now and backtests wait until enough data exists. | Planning, recorder/capture trigger decisions, future replay after accumulation. | Historical replay or retroactive L2 claims. |

## Evidence And Decisions

| Row | Status | Meaning for this project |
|---|---|---|
| E-001 | SOURCE_PROVEN | NT provides `BacktestEngine` and recommends `BacktestNode`; build orchestration over NT, not a Bolt simulator. |
| E-002 | SOURCE_PROVEN | NT `ParquetDataCatalog` is the replay/backtest projection target. Raw provider payloads are audit input, not canonical replay input. |
| E-003 | SOURCE_PROVEN | Upstream NT supports Hyperliquid HIP-4 outcome markets for data and trading; do not build a Bolt HIP-4 adapter first. |
| E-004 | SOURCE_PROVEN | HIP-4 ordinary outcome orders use NT's standard order path. Backtest/order modeling should stay NT-native. |
| E-005 | SOURCE_PROVEN | HIP-4 instruments are USDH `BinaryOption` side tokens; catalog projection and instrument filters must use NT instrument modeling. |
| E-006 | SOURCE_PROVEN | HIP-4 settlement/userOutcome support exists upstream; settlement behavior must be proved through the selected NT path, not reinvented. |
| E-007 | GAP | HIP-4 live support does not prove historical execution-quality outcome replay. |
| E-008 | USER_ASSUMPTION | Kalshi adapter readiness is assumed; this project still must prove Kalshi data/fidelity/source contracts. |
| E-009 | SOURCE_PROVEN + GAP | Kalshi historical markets/candles/trades/fills/orders are documented; historical L2 order-book replay is not proven. |
| E-010 | SOURCE_PROVEN | NT Tardis replay can produce catalog-compatible Parquet for supported crypto venues. |
| E-011 | SOURCE_PROVEN + DECISION_NEEDED | Tardis Professional replay is strong but expensive; cost is a review lever after fidelity is explicit. |
| E-012 | SOURCE_PROVEN | NT Bybit live market data/execution support exists; Bybit is a venue candidate, while historical data-class claims still need proof. |
| E-013 | SOURCE_PROVEN | NT Polymarket support exists, but public historical coverage can hit API cap/depth limits. |
| E-014 | SOURCE_PROVEN | Polymarket discovery, data, and CLOB APIs are distinct source families; Backtesting Engine raw evidence records and lineage must preserve source-family provenance. |
| E-015 | SOURCE_PROVEN + DECISION_NEEDED | Telonex is a Polymarket historical-data candidate; Plus is personal-use priced and commercial/team use needs license proof. |
| E-016 | SOURCE_PROVEN + DECISION_NEEDED | Goldsky can support Polymarket on-chain/provenance indexing, but it is usage-metered and not a free data-lake substitute. |
| E-021 | SOURCE_PROVEN | Hyperliquid perps live data/trading support exists upstream; historical replay still needs data-source proof. |
| E-022 | GAP | Official API/archive capture is not source-proven per venue; Bybit is one unresolved instance, not an architecture anchor. |
| E-024 | USER_ASSUMPTION + DECISION_NEEDED | Choose the best-fidelity architecture first; model cost for review instead of weakening design prematurely. |
| E-026 | SOURCE_PROVEN | Venue/product/provider identity is configuration and registry data, not core code. |
| E-027 | SOURCE_PROVEN + DECISION_NEEDED | Bolt pins NT, but manifest/crate enablement is a separate implementation gate. |
| E-029 | SOURCE_PROVEN | Live credentials must remain AWS SSM-only; this project must not add env-var, CLI, or alternate secret paths. |
| E-030 | SOURCE_PROVEN + DECISION_NEEDED | MarketLens, PMXT, PolyBackTest, PolymarketData, and Goldsky are candidates only after schema/license/sample/NT-mapping proof. |
| E-032 | SOURCE_PROVEN + DECISION_NEEDED | NT exposes configurable backtest engine, venue, run, and simulation surfaces; hidden defaults are not acceptable. |
| E-033 | USER_ASSUMPTION + DECISION_NEEDED | Kimchi premium is a required cross-market source family for `perps/spot` inputs; Korean spot venues such as Upbit/Bithumb are candidate bindings, not hardcoded branches. |
| E-034 | USER_ASSUMPTION + DECISION_NEEDED | Raw payloads, NT catalog, source proofs, and backtest outputs must share one TOML/config-owned S3 `artifact_root` with typed subpaths. |
| E-035 | USER_ASSUMPTION + DECISION_NEEDED | Artifact retention defaults to forever; lifecycle may move artifacts colder but must not delete canonical artifacts by default; archive-as-zero planning threshold is inherited from the reference Artifact Lifecycle Contract. |
| E-036 | USER_ASSUMPTION + DECISION_NEEDED | Lifecycle is simple: artifacts start `active`; after the configured quiet window passes, they become `inactive`; inactive allows archive transition, not deletion. |
| E-037 | SOURCE_PROVEN + GAP | NT upstream supports remote/object-store catalog paths behind storage features, but the currently resolved `bolt-v2` `nautilus-persistence` dependency must prove S3 feature enablement and catalog read/write/query behavior before implementation relies on direct S3 catalog access. |
| E-038 | SOURCE_PROVEN + DECISION_NEEDED | Artifact discovery should use artifact-local manifests, immutable index events, committed snapshots, and generated per-kind latest pointers; event/snapshot serialization remains proof-gated. S3 conditional-write support or an approved commit coordinator must be proved before relying on the index commit path. |
| E-039 | USER_ASSUMPTION + DECISION_NEEDED | Artifact Index write authority is producer-owned; Backtesting Engine publishes records for artifacts it produces, while Research Analytics and Dashboard consume upstream records read-only. |
| E-040 | USER_ASSUMPTION + DECISION_NEEDED | `SourceProofReport` is a Bolt-owned thin gate required before source data becomes canonical NT catalog or backtest input; Backtesting Engine/source-proof implementation owns acceptance, automated acceptance is allowed from initial implementation when all robust checks pass, accepted records are immutable/superseded by new versions, normal runs cannot pin non-latest proof, and non-latest proof pins require structured reason fields. |
| E-041 | SOURCE_PROVEN + DECISION_NEEDED | Backtest result contracts are objective evidence/lookup artifacts; strategy promotion or escalation status belongs to Research Analytics, not the Backtesting Engine result object. |

## Data Source And Fidelity Rules

- Venue/provider names below are candidate data-source bindings only. They must
  not become fixture names, module names, branching rules, or implementation
  architecture.
- Tardis is the strongest NT-native crypto/perps replay candidate where venue,
  cost, and data-class proof pass.
- Official venue archives/APIs are candidates only after per-venue source,
  freshness, schema, and completeness proof.
- Forward capture cannot backfill historical L2 claims.
- Polymarket official APIs are useful for discovery, current order books,
  WebSocket book/deltas/trades, trades, and price history; full historical L2
  replay still needs cap/depth proof or a supplemental source.
- Kalshi uses the user-assumed adapter premise, but data fidelity remains
  independent: historical trades/candles/fills are not historical L2.
- Hyperliquid HIP-4 must split live adapter support from historical outcome data.
- Cost is modeled after best-fidelity options are named; cost does not weaken
  the architecture before user review.
- OKX official historical data, Hyperliquid S3 archive, Binance Data Vision,
  Bybit official docs/data, and paid vendors such as Kaiko, CoinAPI, and
  Amberdata are additional candidates. Each needs the same fidelity, license,
  schema, sample, and NT-mapping proof before selection.
- Kimchi premium sources belong to the `perps/spot` fixture as cross-market
  signal inputs: TOML-selected Korean spot price source(s), reference
  spot/perps price source(s), and FX/quote conversion source(s). Upbit/Bithumb
  are candidate source bindings only.

## Initial Fidelity Matrix

This matrix is a candidate-source evidence table, not a fixture list. Future
implementation starts from market-structure fixtures: `binary option` and
`perps/spot`.

| Candidate source family | Candidate source | Current class | Required next proof |
|---|---|---|---|
| NT backtest core | NT `BacktestNode`, `BacktestEngine`, `ParquetDataCatalog` | `L2_REPLAY` capable when input data supports it | Compile the NT version resolved by the target `bolt-v2` branch and prove API/config mapping. |
| Hyperliquid HIP-4 live | NT upstream Hyperliquid adapter | Live support source-proven; historical class separate | Prove target `bolt-v2` branch support and historical outcome data. |
| Hyperliquid HIP-4 history | Official archive/API, Tardis, or forward capture | `FORWARD_CAPTURE_PENDING` until outcome L2/fill history is proven | Check outcome-market coverage in official archive/Tardis. |
| Kalshi official historical | Kalshi historical API | `TRADE_BAR_REPLAY` or `SIGNAL_ONLY`; not `L2_REPLAY` yet | Prove whether historical orderbook snapshots/deltas exist. |
| Polymarket official APIs | Gamma, CLOB, Data API | `TRADE_BAR_REPLAY` or `SIGNAL_ONLY` until cap/depth proof | Prove public API pagination/depth limits and NT loader behavior. |
| Polymarket Telonex | Telonex Parquet files | `L2_REPLAY` candidate for snapshots; `TRADE_BAR_REPLAY` for trades/quotes | Sample Parquet to NT catalog projection; license gate. |
| Polymarket MarketLens | MarketLens historical orderbook API | `L2_REPLAY` candidate | Sample history endpoint; map snapshots/deltas to NT catalog projection. |
| Polymarket/Kalshi PMXT | PMXT hourly Parquet archive | `L2_REPLAY` candidate for archived snapshots | Validate schema, coverage, gaps, file size/storage, and license/support. |
| Polymarket PolyBackTest | PolyBackTest API | `L2_REPLAY` candidate for supported crypto up/down markets; retention-limited | Verify plan, 31-day retention, market coverage, API schema, and export path. |
| PolymarketData | PolymarketData API/export | `L2_REPLAY` candidate by paid tier | Verify API docs, retention, export, license, and sample. |
| Polymarket Goldsky | Goldsky subgraph/Mirror/Turbo | `SIGNAL_ONLY` or provenance supplement | Estimate events/storage and pair with orderbook source if needed. |
| Perpetual futures Tardis | NT live adapter + Tardis replay | `L2_REPLAY` candidate | TOML/registry binding proof plus replay-to-catalog sample. |
| OKX official historical | OKX historical data download | `L2_REPLAY` candidate | Download/sample schema; map to NT data classes. |
| Hyperliquid official archive | Hyperliquid S3 archive | `L2_REPLAY` candidate for covered assets; HIP-4 coverage unproven | Check HIP-4 outcome symbols/data types and sample archive file. |
| Binance Data Vision | Binance public data | `TRADE_BAR_REPLAY` candidate | Search official depth archive proof or keep lower fidelity. |
| Kimchi premium / Korean spot prices | TOML-selected Korean spot prices such as Upbit/Bithumb plus reference price and FX/quote source | `SIGNAL_ONLY` as premium feature unless component sources prove stronger replay fidelity | Prove source availability, schema, sample, license, token mapping, event/availability time, FX/reference source, and point-in-time join. |
| Bybit official docs/data | Bybit V5/current data and historical data page | `GAP` for historical L2; current snapshot proven | Prove historical data download schema and retention. |
| Kaiko/CoinAPI/Amberdata | Vendor historical orderbook/trades APIs | `L2_REPLAY` candidate by product | Sample vendor payload; map to NT data classes; model license/cost. |

## Data Model

- `BacktestingRunManifest`: venue/provider binding keys, instrument ids, data
  classes, time range, strategy config ref, artifact root ref, catalog artifact
  pointer, output artifact pointer, source proof id/version or explicit proof
  pin, run purpose, required `proof_pin_reason_code` for non-latest proof pins,
  conditional `proof_pin_reason_detail`, lineage hash, fidelity class, and
  extension-surface resolutions.
- `SourceProofReport`: market-structure fixture, TOML/registry binding,
  official/free candidates, paid/vendor candidates, historical order-book
  snapshot/delta availability, retention, freshness, schema, sample pointer,
  license boundary, NT data-class mapping, cross-market reference/FX source
  proof when applicable, fidelity class, forbidden claims, cost, selection
  status, accepted_by, accepted_at, acceptance_mode, and required-check results.
  Accepted records are immutable and may be superseded by a new proof version.
  New runs default to the latest accepted proof unless explicitly pinned in the
  manifest with an allowed `proof_pin_reason_code`. The report is a thin proof
  gate; heavy raw/catalog/result payloads stay in their artifact paths.
- `BacktestExtensionSurface`: surface name, NT reference, classification,
  resolved default, manifest field, custom contract, truth boundary, claim
  limits, and proof required.
- `ExecutionModel`: NT-owned fill, fee, slippage, latency, margin, leverage,
  queue, liquidity, settlement, and order-behavior selection.
- `CatalogProjection`: raw evidence records, NT pointer, catalog path, data
  class, instrument ids, transform hash, and fidelity class.
- `BacktestResultContract`: NT result/report pointers, metrics artifact
  pointers, source hashes, source proof ids, catalog hash, strategy config hash,
  run purpose, fidelity class, claim limits, warnings, and mechanical blockers.
  It must not contain subjective strategy promotion or escalation
  recommendations.
- `ArtifactRoot`: single configured S3 root plus typed subpaths for raw
  payloads, NT catalog projections, source proofs, and backtest outputs.
- `ArtifactIndex`: artifact-local manifests, immutable index events, committed
  snapshots, generated latest pointer, hash rules, producer ownership, write
  authority, commit state, and active hot-path lifecycle status for artifact
  discovery. Exact manifest/event/snapshot/pointer formats and names are
  proof-gated during implementation.
- `ArtifactLifecycle`: retention policy, storage profile, transition windows,
  quiet window, lifecycle state, restore expectation, and delete/expiration
  prohibition for canonical artifacts.

## Extension-Surface Policy

Every relevant NT/custom backtesting surface must be classified before a run is
accepted:

- `defaulted`: use NT default and record the resolved value.
- `pass_through`: expose NT config through manifest after NT field mapping.
- `custom_owned`: allow custom logic only through an NT-compatible interface and
  without creating independent execution, fill, PnL, position, account, or
  portfolio truth.
- `unsupported_for_now`: fail fast if requested.

Classification must be recorded for engine config, venue simulation config,
run config, catalog storage/protocol options, strategy selection,
actor/execution-algorithm selection, risk, portfolio, execution, cache,
message bus, streaming, fill, fee, latency, margin, leverage, queue, liquidity,
settlement, and order-behavior surfaces.

## Strategy Source Policy

Backtesting Engine executes configured NT-compatible strategies and records what
ran. It does not design, optimize, research, or promote strategies.

Accepted strategy sources:

- Existing compiled Rust strategy/actor/execution-algorithm registered in
  `bolt-v2`.
- Human-written typed TOML/config validated against schema.
- Typed config generated by a future Research Analytics experiment-result
  promotion-config field/ref.

Not accepted:

- Inline strategy code in the manifest.
- Notebook code as runtime strategy.
- Python strategy runtime path.
- Untracked config blobs.

## Manifest Obligations

The run manifest is the backtest recipe. It must express run intent and config:

- Market-structure fixture: `binary option` or `perps/spot`.
- TOML/registry venue/provider binding key.
- Instruments or market-selection reference.
- Time range.
- Strategy config reference.
- Catalog input reference.
- Execution, fill, fee, latency, and slippage settings or references.
- Extension-surface settings.
- Configured `artifact_root`.
- Output prefix under `artifact_root/backtests/`.

The manifest must not become a data warehouse, source-proof report, result
file, dashboard schema, or research experiment record.

Exact TOML keys and nesting are finalized during Backtesting Engine
implementation after every manifest obligation maps to target NT config fields.

## Artifact Storage Policy

Backtesting Engine writes canonical artifacts only under configured
`artifact_root`.

- `artifact_root` must be a TOML/config-owned S3 URI.
- The bucket and prefix are configured values, not code constants.
- Typed subpaths are `raw/`, `nt-catalog/`, `source-proofs/`, and
  `backtests/`; index records live under `artifact-index/`.
- Backtest run artifacts are written under `backtests/` by run id unless the
  manifest supplies an explicit output prefix under the same `artifact_root`.
- Raw provider/API/archive payloads are referenced under `raw/`.
- NT `ParquetDataCatalog` projections are referenced under `nt-catalog/`.
- `nt-catalog/` stops at the catalog projection root. NT writes its native
  `data/<data_type>/<instrument_id>/...` tree below that root.
- Direct NT catalog access under `nt-catalog/` must be proved against the
  resolved `bolt-v2` NT dependency and crate features before implementation
  relies on S3 for runtime catalog reads/writes/queries.
- Manifest/catalog metadata must record whether direct S3 catalog access is
  proven for the run. Any staging path must be explicit, non-canonical, and
  stamped with the source S3 URI/hash; hidden local fallback is forbidden.
- Source proof reports and samples are referenced under `source-proofs/`.
- No separate canonical root knobs are allowed for raw data, catalog data,
  source proofs, or backtest outputs.
- Local paths are cache/development fixtures only, never canonical artifacts.
- No hidden cwd, temp-directory, or sibling-project fallback path is allowed.
- `BacktestResultContract` records final artifact URIs for NT reports/results,
  summary artifacts, warnings, logs, catalog input, and source proof records.
- Backtesting Engine does not write directly into Research Analytics or
  Dashboard storage by default.

## Artifact Index Policy

Backtesting Engine publishes canonical artifacts through the cross-project Artifact
Index Contract. This remains a thin table of contents for S3 artifacts and must
not become a warehouse, query engine, or replacement for NT `ParquetDataCatalog`.

- Each backtest run writes an artifact-local manifest under its output prefix
  using the selected structured format.
- Each artifact produces an immutable structured index event under
  `artifact-index/v1/events/kind=<artifact_kind>/` or the selected event path
  for that top-level kind.
- Index records for source-proof, catalog projection, and backtest artifacts are
  written only by the producer job for that artifact. Consumers may read those
  records but must not repair, invent, or mutate them.
- Bulk discovery uses the committed snapshot reachable from the generated
  per-kind latest pointer.
- The latest pointer is generated and updated only through the approved
  conditional commit path at
  `artifact-index/v1/pointers/kind=<artifact_kind>/latest.json`. It is not
  manually maintained.
- Backtesting Engine readers must not independently join two per-kind latest snapshots. To find
  the source proof, catalog projection, or raw inputs used by a backtest, follow
  the backtest manifest lineage ids/version/hash and verify `sha256`.
- Every Backtesting Engine-produced event and snapshot row must carry parent cross-kind
  lineage ids, versions where applicable, and `sha256` content hashes.
- Events or artifacts not reachable from the snapshot referenced by
  the latest pointer are staged/orphan audit input, not committed discovery
  truth.
- Recursive S3 listing is forbidden for normal discovery. Listing is allowed
  only for off-path reconciliation, recovery, and compaction.
- The current latest pointer, current snapshot, and metadata needed to resolve
  the current snapshot must remain in active/queryable storage.
- Pointer swaps append audit epoch records for forensics only. Normal discovery
  still uses the per-kind latest pointer and snapshot.
- Backtest callers may use the returned `BacktestResultContract` immediately as
  an artifact-local handle; cross-run consumers use the committed Artifact Index
  snapshot.

## Artifact Lifecycle Policy

- Default retention is forever.
- Default delete/expiration is disabled.
- Required storage profiles are `active`, `archive`, and `deep_archive`.
- Transition windows are TOML/config-owned, not code constants.
- Scratch or failed runs may be tagged for faster archive transition, but not
  automatic deletion.
- Lifecycle state starts as `active`.
- After the configured quiet window passes, lifecycle state becomes `inactive`.
- `inactive` permits archive/deep-archive transition, not deletion.
- Archive-as-zero planning threshold is inherited from the
  [Artifact Lifecycle Contract](../reference/contracts.md); restore,
  retrieval, and minimum-duration costs remain explicit.
- Future implementation must finalize exact quiet-window values and timestamp
  basis.
- Restoring archive/deep-archive artifacts is an explicit operational step and
  must be visible in result/source-proof metadata when relevant.

## Result Contract Obligations

Backtest results must preserve these obligations:

- Identify the run.
- Identify the target `bolt-v2` branch/ref and NT version resolved by that
  branch.
- Identify the manifest and strategy config used for the run.
- Identify the source proof report and NT catalog input used for replay.
- Identify the market-structure fixture.
- Carry fidelity class and claim limits.
- Point to NT reports/results as the result truth source.
- Carry warnings or gaps that limit interpretation.
- Record creation time for audit/freshness.

Exact field names, IDs, hash formats, and artifact-pointer shapes must be
finalized during Backtesting Engine implementation after inspecting the selected
NT output shape.

## Issue Dependencies

Current dependency status is recorded in
`../reference/issue-dependency-status.backtesting-engine-039.2026-06-09.json`.
The listed GitHub issues are live scope boundaries for implementation review,
not closure claims.

Link, update, or depend on:

| Issue | Backtesting Engine relation |
|---|---|
| [#19](https://github.com/seungpyoson/bolt-v2/issues/19) | Raw/catalog/result lineage context; do not claim normalized-lake lineage closure. |
| [#23](https://github.com/seungpyoson/bolt-v2/issues/23) | Instrument/catalog completeness context; do not claim instrument-spool bridge closure. |
| [#24](https://github.com/seungpyoson/bolt-v2/issues/24) | Parent NT-first data-lake scope; avoid redefining canonical lake layout. |
| [#34](https://github.com/seungpyoson/bolt-v2/issues/34) | Strategy-platform consumer context; result contracts must not encode promotion decisions. |
| [#112](https://github.com/seungpyoson/bolt-v2/issues/112) | Kalshi venue/source context; source proof is required before any catalog/backtest use. |
| [#115](https://github.com/seungpyoson/bolt-v2/issues/115) | HIP-4 venue context; historical replay claims remain source-proof gated. |
| [#127](https://github.com/seungpyoson/bolt-v2/issues/127) | Polymarket native depth constraint; do not overclaim `OrderBookDepth10`. |
| [#148](https://github.com/seungpyoson/bolt-v2/issues/148) | Capture-isolation risk context; provider recorder/live capture expansion stays out of scope. |
| [#158](https://github.com/seungpyoson/bolt-v2/issues/158) | Analytics-adjacent sidecar source context; BTE records gaps but does not build collectors. |
| [#236](https://github.com/seungpyoson/bolt-v2/issues/236) | Thin NT architecture parent; no custom simulator or dual catalog path. |
| [#254](https://github.com/seungpyoson/bolt-v2/issues/254) | Polymarket V2 readiness context; one-off PMXT evidence is not production V2 readiness. |
| [#407](https://github.com/seungpyoson/bolt-v2/issues/407) | Controlled discovery boundary; source/instrument coverage must stay TOML/proof bounded. |

Do not claim this project closes those broader issues unless its implementation
actually satisfies their accepted scope. Do not create or mutate GitHub issues
from this spec without explicit user approval.

## Non-Goals

- No provider recorder, data lake writer, or live capture expansion.
- No Research Analytics notebook or experiment workflow.
- No Dashboard UI/read model.
- No live order submit/cancel/transfer path.
- No Kalshi adapter implementation.
- No Bolt-owned simulator unless NT is source-proven unable to provide the
  required surface.

## Acceptance

- Reviewer can map every manifest field to an NT config field or explicit Bolt
  orchestration metadata.
- Reviewer can see one defaulted surface, one pass-through NT config surface,
  and one custom-owned or unsupported surface in fixtures.
- Reviewer can swap venue/provider binding through TOML/registry data only.
- Lower-fidelity data cannot produce execution-quality claims.
