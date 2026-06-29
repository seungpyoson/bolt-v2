# Plan: Research Analytics

## Architecture

```text
RawEvidenceRecord / CatalogProjection / NT Result / NT Report
  -> ResearchDataset
  -> point-in-time feature joins
  -> ExperimentRun
  -> analysis artifacts and metrics
  -> ExperimentResult verdict + optional typed promotion-config
```

Research Analytics is flexible at the exploration layer, but promotion is strict:
production behavior must graduate into typed TOML/NT-compatible runtime
contracts.

## Source Rules

- Raw evidence is audit input.
- NT catalog projection is deterministic replay/backtest input.
- NT reports/results/events/snapshots are trading-state source data.
- Analytics tables are derived and must carry source hashes and freshness.
- Exploratory sources must be labeled non-trading-truth.
- Kimchi premium sources require separate Korean spot, reference price, and
  FX/quote source proofs; Upbit/Bithumb-style sources are candidate TOML
  bindings, not hardcoded analytics branches.
- Canonical raw payloads, NT catalog data, source proof artifacts, and backtest
  outputs are read by URI from the configured S3 `artifact_root`; analytics must not
  fork those artifacts into a second canonical root.
- Bulk artifact discovery uses the committed Artifact Index snapshot under the
  configured `artifact_root`; analytics must not recursively scan S3 prefixes as its
  normal discovery path.
- Direct producer/caller handoffs may pass explicit artifact-local handles;
  those handles do not replace the committed snapshot path for cross-run
  discovery.
- Analytics is read-only for upstream raw, NT catalog, source-proof, and
  backtest Artifact Index records. Derived research artifacts require explicit
  RA-owned artifact kind/schema before Analytics can publish index records.
- RA-owned derived artifacts use one top-level Artifact Index kind,
  `research-analytics`, with three subfamilies: `datasets`, `feature-tables`,
  and `experiment-results`. These subfamilies commit into one
  `research-analytics` snapshot and do not get separate latest pointers.
  Promotion config is a typed field/URI on an `experiment-results` artifact,
  not a separate artifact family/path.
- Analytics preserves upstream `SourceProofReport` ids, fidelity classes, and
  claim limits through datasets, experiments, and verdict-bearing experiment
  results. It may
  narrow claims but cannot accept upstream proof, upgrade proof strength, or
  weaken forbidden claims. It preserves proof version/supersession metadata and
  cannot mutate accepted proof records. Historical experiments remain tied to
  the proof version they consumed; supersession does not relabel old results.
  Non-latest proof pins preserve upstream `proof_pin_reason_code` and
  `proof_pin_reason_detail` when present. Analytics preserves upstream
  `run_purpose` so normal and reproduction/audit/regression/migration results
  remain distinguishable. RA experiment runs consuming non-latest proof or
  pinned backtests must carry the same non-normal run purpose and structured pin
  reason fields. This rule is scoped to non-latest `source_proof_version`, not
  every older versioned component such as NT version, strategy config, catalog
  hash, manifest schema, or historical data window; those require separate
  future currentness rules deferred to manifest-schema work.
- Artifact lifecycle metadata is preserved from source through datasets,
  experiments, and experiment results. Analytics cannot add default delete or
  expiration rules for canonical artifacts.
- Lifecycle state remains simple for analytics consumers: `active` until the
  configured quiet window passes, then `inactive`.

## Implementation Gates

1. Define dataset/source lineage schema.
2. Define point-in-time join and leakage-check rules.
3. Define experiment metadata and artifact retention.
4. Define claim-limit propagation from source fidelity to research result.
5. Define notebook permission boundary.
6. Define findings, promotion-config fields, and review checklist.
7. Define artifact URI and Artifact Index consumption rules for the configured S3
   `artifact_root`.
8. Define lifecycle metadata rules for retain-forever artifacts, quiet window,
   and active-to-inactive transition.
9. Define cost refresh and provider/license proof triggers for selected data.

## Backtest Phase Prerequisite

The BTE runner currently registers only the NT example strategy
`HurstVpinDirectional` over `bybit-spot`. Wiring bolt's binary-oracle
edge-taker strategy (registry key `STRATEGY_BINARY_ORACLE_EDGE_TAKER` in
`crates/backtesting-vertical-slice/src/run_manifest.rs`) plus venue
normalization into the BTE is a hard precondition before any Phase-3 sweep
is real.

## Point-In-Time Rules

- Every feature must declare event time, availability time, and join key.
- Joins must use as-of semantics, never future observations.
- Dataset snapshots must carry source hashes and query/config hashes.
- Research output must preserve source fidelity and forbidden claims.
- Cross-market premium features must align Korean spot price, reference price,
  and FX/quote observations by event time and availability time.

## Promotion Rules

- Notebook code cannot be promoted directly.
- A promoted candidate must become typed TOML/NT-compatible config plus runtime
  contract.
- `BacktestResultContract` is an objective evidence input only. Strategy
  escalation, candidate status, rejection, or approval belongs to the
  `experiment-results` verdict field set or a later RA-owned review artifact.
- A real `go` finding may carry typed TOML/NT-compatible promotion-config
  fields on the same `experiment-results` artifact. `go` is not live-trading
  approval.
- Promotion config requires accepted `SourceProofReport` refs, objective
  backtest result refs, preserved claim limits, fidelity-compatible claims, no
  notebook runtime code, typed TOML/NT-compatible config output,
  reviewer/policy refs, and explicit non-live boundary.
- The only allowed output is a typed config field/URI on the
  `experiment-results` artifact for later implementation/review. It must not
  auto-merge, auto-enable a strategy, schedule live trading, touch SSM
  credentials, mutate production runtime config, or create a separate promotion
  artifact family/path.
- Promotion must name required Backtesting Engine evidence and selected
  Dashboard source fields, if any, using the experiment-result verdict and
  promotion-config fields defined in the project spec.
- Promotion must not bypass SSM-only live credential handling or Rust-only
  production runtime rules.

## Issue Payload

Title: `Plan: NT-derived research analytics and alpha workflow spec`

Accepted scope: define raw evidence and deterministic projection lineage,
point-in-time correctness, leakage checks, experiment metadata, notebook
boundary, and promotion path from research finding to typed TOML/NT-compatible
runtime contract.

Out of scope: building analytics DB/read model, notebook implementation, strategy
productionization, and replacing NT reports.

## Test Plan

- Leakage fixtures fail when future data is joined.
- Experiment manifests fail when source hashes or as-of bounds are missing.
- Notebook boundary checks fail on production mutation capabilities.
- Promotion package validation fails without typed config/runtime contract.
- Promotion package validation fails without explicit evidence references for
  backtest inputs/results and dashboard-facing fields.

## Residual Risks

- Exact query/read-model tooling is still decision-needed.
- Vendor/source schemas and licenses must be refreshed before selection.
- Research results can overclaim if fidelity labels are dropped.
- Promotion can become a shadow runtime unless typed config review is enforced.
