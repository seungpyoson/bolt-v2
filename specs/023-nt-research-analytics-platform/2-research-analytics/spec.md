# Spec: Research Analytics

## Scope

Build a future research-only analytics project over raw evidence, NT catalog
projections, and NT-derived backtest/live results. It owns experiments,
notebooks, point-in-time correctness, feature/result lineage, and promotion
gates into typed runtime configuration.

This is separate from Backtesting Engine and Dashboard. It does not own the NT
runner, live trading, dashboard UI, provider procurement, or independent
PnL/account truth.

## Users

- Researcher: explores data, features, strategies, and results.
- Maintainer: reviews experiment lineage, leakage controls, and promotion
  packages before anything can affect production runtime.

## Requirements

- Every feature/result must trace to raw evidence, catalog projection, NT result,
  NT report, or explicitly exploratory source.
- Research notebooks may read data and produce analysis artifacts only.
- Notebook/Python workflows must not submit orders, cancel orders, transfer
  funds, mutate credentials, or become production runtime.
- Feature joins must be point-in-time correct and carry as-of/freshness rules.
- Experiments must record parameters, datasets, hashes, metrics, artifacts,
  fidelity class, and claim limits.
- Promotion to production requires typed TOML/NT-compatible config and runtime
  contract, not notebook code.
- Promotion packages may produce typed strategy config for Backtesting Engine,
  but they do not create a Python strategy runtime path.
- Analytics/read models must not become independent PnL, position, account, or
  portfolio truth.
- Venue/provider identity remains TOML/registry-selected data, not hardcoded
  analytics branches.
- Kimchi premium features must use TOML/registry-selected Korean spot price
  source(s), reference spot/perps price source(s), FX/quote conversion
  source(s), and point-in-time joins.
- Research datasets must reference canonical raw, NT catalog, source-proof, and
  backtest artifacts under the configured S3 `artifact_root`.
- Research datasets and experiment outputs must preserve upstream
  `SourceProofReport` ids, fidelity classes, and claim limits. Analytics may
  narrow claims but must not upgrade source/backtest fidelity.
- Analytics must not mark upstream `SourceProofReport` records accepted or
  weaken forbidden claims for catalog/backtest use.
- Analytics must preserve source proof version/supersession metadata and must
  not mutate accepted proof records.
- Analytics must keep historical backtest/experiment records tied to the proof
  version they used; supersession metadata may be shown but must not relabel
  old results as if they used the newer proof.
- When consuming runs pinned to a non-latest proof, analytics must preserve the
  upstream `proof_pin_reason_code` and `proof_pin_reason_detail` when present.
- Analytics must preserve upstream `run_purpose` so normal results are not mixed
  with reproduction/audit/regression/migration results without labels.
- Research Analytics experiment runs that consume non-latest proof or pinned
  backtests must carry the same non-normal `run_purpose` and structured pin
  reason fields; they must not publish such experiments as normal current
  results.
- This non-normal requirement applies specifically to non-latest
  `source_proof_version`. It does not automatically classify older NT versions,
  strategy config hashes, catalog hashes, manifest schema versions, or
  historical data windows; those require separate future currentness rules
  deferred to manifest-schema work.
- Research datasets may consume explicit artifact-local handles passed by a
  producer/caller; cross-run and bulk artifact discovery must use committed
  Artifact Index snapshots, not recursive S3 listing.
- Research Analytics is read-only for upstream raw, NT catalog, source-proof,
  and backtest Artifact Index records. If it later produces derived research
  artifacts, it may write only those RA-owned artifact records.
- RA-owned derived artifacts use the single top-level `research-analytics`
  Artifact Index kind. Subfamilies are `datasets`, `feature-tables`,
  and `experiment-results`; they do not get separate latest pointers. Promotion
  config is a typed field/URI on an `experiment-results` artifact when a real
  GO finding exists, not a separate artifact family.
- Research datasets must preserve artifact lifecycle metadata and must not
  propose default deletion of canonical artifacts.

## Evidence And Decisions

| Row | Status | Meaning for this project |
|---|---|---|
| E-002 | SOURCE_PROVEN | NT catalog projection is the replay/backtest data basis analytics should reference. |
| E-014 | SOURCE_PROVEN | Polymarket discovery, data, and CLOB sources are separate source families; lineage must preserve provenance. |
| E-015 | SOURCE_PROVEN + DECISION_NEEDED | Telonex is a Polymarket historical-data candidate; Plus is personal-use priced and commercial/team use needs license proof. |
| E-016 | SOURCE_PROVEN + DECISION_NEEDED | Goldsky can support Polymarket on-chain/provenance indexing, but it is usage-metered and should be selected only after event/storage/query estimates. |
| E-017 | SOURCE_PROVEN | NT reports/events/snapshots can be analytics inputs for trading-state-derived metrics. |
| E-020 | SOURCE_PROVEN | Existing issues overlap with data lake, strategy, and readiness work; do not create one broad duplicate issue. |
| E-024 | USER_ASSUMPTION + DECISION_NEEDED | Model best-fidelity data/product choices first, then expose all-in cost for user review. |
| E-025 | SOURCE_PROVEN | Python/Jupyter is research-only and cannot become the production trading runtime. |
| E-026 | SOURCE_PROVEN | Venue/product/provider identity is TOML/registry-selected data. |
| E-029 | SOURCE_PROVEN | Live credentials must remain AWS SSM-only; analytics and promotion flows must not introduce alternate secret sources. |
| E-030 | SOURCE_PROVEN + DECISION_NEEDED | MarketLens, PMXT, PolyBackTest, PolymarketData, and Goldsky are candidates after schema/license/sample proof. |
| E-031 | SOURCE_PROVEN | Lean, Qlib, Freqtrade, and Feast support separation of research/backtest/live lifecycle and leakage controls as prior art. |
| E-033 | USER_ASSUMPTION + DECISION_NEEDED | Kimchi premium is a required cross-market feature/source family; Upbit/Bithumb-style Korean spot prices are candidate bindings, not hardcoded analytics branches. |
| E-034 | USER_ASSUMPTION + DECISION_NEEDED | Analytics consumes raw, NT catalog, source-proof, and backtest artifact pointers under the configured S3 `artifact_root`; it must not create a second canonical storage root for the same artifacts. |
| E-035 | USER_ASSUMPTION + DECISION_NEEDED | Analytics must preserve retain-forever lifecycle metadata and cannot introduce default artifact deletion. |
| E-036 | USER_ASSUMPTION + DECISION_NEEDED | Analytics preserves the simple lifecycle state: artifacts start `active`; after configured quiet window they become `inactive`; inactive allows archive transition, not deletion. |
| E-038 | SOURCE_PROVEN + DECISION_NEEDED | Analytics bulk discovery must consume the committed Artifact Index snapshot and must not scan S3 prefixes as its normal discovery path. |
| E-039 | USER_ASSUMPTION + DECISION_NEEDED | Analytics is read-only for upstream raw/catalog/source-proof/backtest Artifact Index records; it may write only explicitly RA-owned derived artifact records. |
| E-040 | USER_ASSUMPTION + DECISION_NEEDED | Analytics consumes `SourceProofReport` ids, fidelity classes, and claim limits as upstream proof metadata; it must not accept upstream proof, weaken forbidden claims, or reclassify weaker sources as stronger evidence. |
| E-041 | SOURCE_PROVEN + DECISION_NEEDED | Backtesting Engine emits objective result contracts only; Analytics owns strategy review verdicts and any generated typed promotion config on `experiment-results`, or later RA-owned review artifacts. |

## Fidelity Class Reference

Research Analytics consumes fidelity labels from Backtesting Engine and data
source contracts. It must preserve them in datasets, experiments, metrics, and
verdict-bearing experiment results.

| Class | Meaning for analytics |
|---|---|
| `L2_REPLAY` | Execution-quality claims may be analyzed only for the proven venue/source/instrument scope. |
| `TRADE_BAR_REPLAY` | Price, alpha, trade/fill, candle, or bar research only; no queue/execution-quality claims. |
| `SIGNAL_ONLY` | Signal, feature, provenance, or dashboard context only. |
| `FORWARD_CAPTURE_PENDING` | Capture may start now; historical analytics cannot claim replay coverage yet. |

## Prior-Art Rules

- Lean: keep research, backtest, optimize, and live promotion as separate
  lifecycle gates.
- Qlib: experiments need dataset, model, parameter, metric, artifact, and
  analysis lineage.
- Freqtrade lookahead analysis: backtest/research validity can fail when signals
  see future data; add leakage fixtures.
- Feast: feature sets need point-in-time semantics.
- Kimchi premium: treat premium as a derived cross-market feature from Korean
  spot price, reference price, and FX/quote sources; never join future reference
  or FX observations into earlier signals.

## Data Model

- `ResearchDataset`: raw evidence records, catalog projections, NT result/report
  references, artifact URIs under configured `artifact_root`, source hashes,
  source proof ids, run purpose, proof pin reason code/detail when present,
  fidelity classes, claim limits, lifecycle metadata, and as-of bounds.
- `RawEvidenceRecord`: source family, source URI or redacted pointer, capture
  time, source time range, payload hash, schema/version, license reference, and
  lineage parent.
- `CatalogProjection`: projection id, source records, NT pointer, catalog path,
  NT data type, instrument ids, transform/config hash, fidelity class, and
  validation status.
- `ExperimentRun`: parameters, code/artifact refs, dataset refs, metrics,
  result hashes, consumed `BacktestResultContract` refs, run purpose, proof pin
  reason code/detail when present, fidelity class, and claim limits.
- `FeatureDefinition`: source fields, join keys, event time, availability time,
  leakage checks, cross-market component source refs when applicable, and
  allowed consumers.
- `ResearchAnalyticsArtifact`: RA-owned artifact under
  `research-analytics/v1/{datasets,feature-tables,experiment-results}/`
  with schema version, source refs, source hashes, `sha256` content hash,
  lifecycle state, Artifact Index event, and owner `research-analytics`.

## Findings And Promotion Config

Strategy review status is recorded on `experiment-results` as a verdict field
set:

- `blocked`: required evidence or validation is missing or failed.
- `changes_requested`: more research, tuning, data proof, reruns, or feature
  work is required before review can proceed.
- `rejected`: reviewed and not accepted.
- `go`: a real finding may carry typed TOML/NT-compatible promotion-config
  fields on the same `experiment-results` artifact; this is not live-trading
  approval.

Any `go` promotion config requires accepted `SourceProofReport` refs, objective
backtest result refs, preserved claim limits, fidelity-compatible claims, no
notebook runtime code, typed TOML/NT-compatible config output, reviewer/policy
refs, and an explicit non-live boundary.

The only allowed output is a typed config field/URI on the
`experiment-results` artifact for later implementation/review. It must not
auto-merge, auto-enable a strategy, schedule live trading, touch SSM
credentials, mutate production runtime config, or create a separate promotion
artifact family/path.

## Known prerequisite

The BTE runner currently registers only the NT example strategy
`HurstVpinDirectional` over `bybit-spot`. Wiring bolt's binary-oracle
edge-taker strategy (registry key `STRATEGY_BINARY_ORACLE_EDGE_TAKER` in
`crates/backtesting-vertical-slice/src/run_manifest.rs`) plus venue
normalization into the BTE is required before Phase-3 sweeps are real.

## Issue Dependencies

Link or depend on #19, #20, #21, #22, #24, #34, #39, #75, #148, #158, #176,
#236, and #407 as applicable. Existing data-lake and strategy issues do not
fully cover research analytics, alpha exploration, or promotion gates.

## Non-Goals

- No NT backtest runner implementation.
- No dashboard UI or operator control plane.
- No live trading or credential mutation.
- No provider recorder or data-lake capture expansion.
- No custom PnL/account truth.

## Acceptance

- Reviewer can reproduce each research result from recorded source hashes and
  parameters.
- Reviewer can identify which claims are execution-quality, lower-fidelity, or
  exploratory.
- Reviewer can prove notebooks have no production mutation path.
- Reviewer can see typed promotion-config fields on the `experiment-results`
  artifact before any production runtime work.
