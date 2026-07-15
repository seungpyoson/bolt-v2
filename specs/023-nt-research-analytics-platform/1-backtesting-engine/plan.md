# Plan: Backtesting Engine

## Architecture

```text
BacktestingRunManifest
  -> venue/provider bindings
  -> NT ParquetDataCatalog projection
  -> NT BacktestRunConfig / BacktestDataConfig / BacktestVenueConfig
  -> NT BacktestNode
  -> NT BacktestResult / reports
  -> BacktestResultContract
```

NT owns the engine, catalog input, simulated venue behavior, order/fill
lifecycle, portfolio/account truth, and reports. Bolt owns manifest validation,
binding selection, orchestration metadata, result packaging, and claim-limit
enforcement.

## Source-Of-Truth Chain

```text
Raw provider/API/archive payload
  -> immutable raw evidence record
  -> deterministic NT catalog projection
  -> NT BacktestNode / NT reports / NT results
  -> result contract for analytics/dashboard consumers
```

Raw evidence is audit input. NT catalog is canonical replay input. NT results
are backtest trading truth. Analytics and dashboard consumers are derived.

## Artifact Storage Policy

One TOML/config-owned S3 `artifact_root` stores canonical raw payloads, NT
catalog data, source proof artifacts, and backtest outputs. The bucket and
prefix are configured values, not code constants.

Standard typed subpaths:

- `raw/`
- `nt-catalog/`
- `source-proofs/`
- `backtests/`
- `artifact-index/`
- `research-analytics/` reserved for downstream RA-owned artifacts

Backtest run artifacts live under `backtests/` by run id unless the manifest
sets an explicit output prefix under the same `artifact_root`. Raw source
payloads, NT catalog projections, source proofs, backtest results, and RA-owned
derived artifacts must not define separate canonical roots. Local filesystem
paths are cache or development fixtures only. There is no hidden cwd,
temp-directory, or sibling-project fallback.

Paths use short config-selected source bindings, market-structure fixture
labels, artifact ids, date partitions where useful, and artifact-local
manifests. Full venue/provider/instrument/license/time details live in
manifests, source proofs, and index metadata. The `nt-catalog/` prefix stops at
the catalog projection root and lets NT write its native
`data/<data_type>/<instrument_id>/...` tree below that root.

Artifact discovery uses the cross-project Artifact Index Contract. The index is a thin
table of contents: artifact-local structured manifests, immutable structured
events, committed snapshots, and generated per-kind latest pointers at
`artifact-index/v1/pointers/kind=<artifact_kind>/latest.json`. Event and
snapshot serialization choices remain proof-gated. Normal readers do not
recursively list S3 to find artifacts. Index writes are producer-owned: the job
that creates a canonical artifact publishes its index record, while consumers
read upstream records without mutating them. Cross-kind reads follow manifest
lineage ids/version/hash; they do not join independently read latest snapshots.

Historical backtest input is stricter than bulk discovery. A run selects
explicit dataset-manifest URI and digest values, verifies the exact versioned
objects, and passes NT a sealed local catalog view. The per-kind latest pointer
cannot select, substitute, or advance run input, and a raw S3 catalog URI is
not a production binding path.

Lifecycle policy:

- Retain canonical artifacts forever by default.
- Never configure default delete/expiration lifecycle rules.
- Use storage profiles: `active`, `archive`, `deep_archive`.
- Keep transition windows in TOML/config.
- Archive inactive artifacts instead of deleting them.
- Require explicit future approval for any purge/delete policy.
- Lifecycle state starts as `active`; after the configured quiet window passes,
  it becomes `inactive`.
- Archive storage under `$5/month` is zero for planning, while restore and
  retrieval costs remain explicit.
- Future implementation must finalize exact quiet-window values and timestamp
  basis per artifact kind.

## Implementation Gates

1. Prove the NT version resolved by the target `bolt-v2` branch and required
   crates/features compile in Bolt.
2. Prove the resolved `bolt-v2` NT dependency can read/write/query
   `ParquetDataCatalog` data from the configured S3 `artifact_root`, including
   required crate features and storage options. If direct S3 catalog access is
   not supported, document the supported staging path before implementation.
3. Prove the configured artifact store supports the Artifact Index commit path:
   immutable create-only writes, per-kind conditional latest-pointer update,
   snapshot hash verification, retry/rebase on conditional-write failure,
   staged/orphan recovery, producer-owned write authority, read-only consumer
   enforcement, event/snapshot format selection, cross-kind lineage traversal,
   `sha256` content hashes, audit epoch append, per-kind IAM, and active storage
   for the hot index path. If
   unsupported, select an approved commit coordinator or table format before
   relying on the index.
4. Prove `ParquetDataCatalog` input path and required NT data classes for the
   selected venue/provider fixtures.
5. Prove manifest mapping into NT config structs.
6. Prove extension-surface classification and resolved-default recording.
7. Prove result contract carries objective NT pointer, source hashes, fidelity,
   claim limits, warnings, and mechanical blockers without encoding strategy
   promotion or escalation decisions.
8. Prove venue/provider swap is TOML/registry-only.
9. Refresh provider license/schema/sample/cost proof at selection time.

## Extension Surface Policy

- `defaulted`: use NT default and write resolved value to the run manifest.
- `pass_through`: expose NT config directly through TOML/manifest.
- `custom_owned`: plug a custom implementation into an NT-compatible interface;
  NT remains trading truth unless the result is explicitly exploratory.
- `unsupported_for_now`: reject the request before running.

Required coverage includes engine cache/msgbus/data/risk/execution/portfolio/
streaming/logging/timeouts/analysis, venue fill/fee/latency/margin/leverage/
routing/order behavior/bar-trade execution/liquidity/queue/settlement, run
chunking/start/end/disposal/exception behavior, catalog storage options, and
strategy/actor/execution-algorithm selection.

## Provider Selection Procedure

Run this procedure per market-structure fixture. The fixture names are
`binary option` and `perps/spot`; any concrete venue/provider is selected only
through TOML/registry binding data.

Proof order is fixed:

1. Check official/free sources first.
2. If fidelity is insufficient, evaluate paid/vendor sources.
3. If no usable history exists, mark `FORWARD_CAPTURE_PENDING`.

Use L2 order-book data when a source passes proof. Weaker data is allowed only
with claim limits that block execution-quality claims.

Candidate discovery is mechanical, not limited to the examples in this package:

1. Register each candidate with fixture type, source binding key, source family,
   official/free or paid/vendor class, target coverage, target time range, and
   intended fidelity claim.
2. Fetch a proof packet: docs/license ref, schema ref, representative sample URI
   and hash, coverage window, retention/freshness facts, and cost terms if paid.
3. Run required checks: license, sample access, schema, time semantics, coverage,
   NT mapping or approved signal-input mapping, fidelity, forbidden claims, and
   cost if paid/vendor.
4. Classify the candidate as `ACCEPTED_FOR_REQUIRED_FIDELITY`,
   `ACCEPTED_LOWER_FIDELITY`, `REJECTED`, or `PENDING_MORE_PROOF`.
5. Paid/vendor candidates are considered only for gaps recorded from
   official/free candidates. `FORWARD_CAPTURE_PENDING` is allowed only after
   official/free and paid/vendor candidates fail required historical fidelity.
6. Select the highest accepted fidelity source; if candidates tie, prefer the
   clearer license, schema, sample, coverage, cost, and NT mapping proof.

Hard rejection or non-selection criteria: no obtainable sample, unclear or
prohibited license, missing or non-inferable schema, missing event/availability
time basis, no target coverage, no NT catalog or approved signal-input mapping,
unsupported L2 claim, required hardcoded venue/provider branch, non-SSM
credential path, storage outside configured `artifact_root`, or unestimated
cost for review.

1. Create `SourceProofReport` for the fixture before provider selection.
2. Compare official/free candidates and paid/vendor candidates in that report.
3. Prove historical order-book snapshot/delta availability, retention,
   freshness, schema, and sample access.
4. Prove license and commercial/personal use boundary.
5. For kimchi premium or other cross-market signals, prove Korean spot source,
   reference price source, FX/quote source, token mapping, and point-in-time
   availability before use.
6. Prove selected samples and transformed artifacts resolve under the configured
   S3 `artifact_root` typed subpaths.
7. Prove lifecycle metadata: retention forever, storage profile, quiet window,
   active-to-inactive transition rule, and no default delete rule.
8. Map one sample into NT-compatible data classes or approved signal input.
9. Assign fidelity class and forbidden claims.
10. Estimate subscription, AWS storage/compute/transfer, query/log, and reserve
   costs for backtesting data and replay.
11. Select source only after source proof, license, sample, NT mapping, fidelity,
   and cost proof are recorded.
12. Present cost cut levers only after the fidelity case is explicit.

`SourceProofReport` is the early-phase source gate. It is Bolt-owned and thin:
it stores source/schema/license/time/fidelity proof pointers, hashes, warning
labels, and claim limits, not heavy payloads or NT catalog data. It must exist
for both `binary option` and `perps/spot` before a provider can be selected, and
an accepted report is mandatory before data becomes canonical NT catalog input
or backtest input. Acceptance for catalog/backtest use is owned by the
Backtesting Engine/source-proof implementation; downstream projects can reject
or narrow their own use but cannot mark proof accepted or upgrade it. Acceptance
automation is allowed from the first implementation when every required schema,
sample, license, time/freshness, NT mapping, fidelity, and forbidden-claim
check passes; missing or ambiguous proof stays pending or rejected. Manual
review is a fallback or override path, not deferred to a later phase. Accepted
reports are immutable; changed facts create a new source proof version that
supersedes the prior accepted proof. Historical backtest results remain tied to
the proof version in their manifest. New runs use the latest accepted proof
unless the manifest explicitly pins an older accepted proof for reproducibility with
`proof_pin_reason_code`. Allowed codes are `baseline_reproduction`,
`published_result_reproduction`, `regression_comparison`,
`audit_or_investigation`, and `migration_validation`; `proof_pin_reason_detail`
is required for `audit_or_investigation` and optional for the other codes.
`run_purpose` is `normal`, `reproduction`, `audit`, `regression`, or
`migration`; `normal` runs cannot pin non-latest proof.
NT version, strategy config hash, catalog hash, manifest schema, and
execution-model currentness are separate future manifest rules, not source-proof
pin policy. Their exact currentness semantics are deferred to manifest-schema
finalization after NT config mapping proof.

## Cost Baselines

These are planning snapshots and must be refreshed before provider selection.
Provider names here are cost evidence for candidate bindings, not architecture
anchors or required fixtures.

| Scenario | Known monthly cost | Status | Backtesting implication |
|---|---:|---|---|
| Tardis Perpetuals Professional | `$900` provider | `OVER_TARGET_REVIEW` once AWS/query/log costs are added | Strong crypto/perps replay candidate; do not reject before fidelity case is explicit. |
| Tardis All Exchanges Professional | `$2200` provider | `OVER_TARGET_REVIEW` | Broadest replay candidate; requires explicit user review before implementation. |
| Telonex Plus | `$79` provider | `DECISION_NEEDED` | Personal-use Polymarket research source only; commercial/team use needs Enterprise quote. |
| Telonex Enterprise | Custom | `DECISION_NEEDED` | Required before commercial/team reliance. |
| Goldsky Starter/Scale | Free starter, usage-metered scale | `DECISION_NEEDED` | On-chain provenance supplement; estimate event/storage/query usage before selection. |
| Hyperliquid official archive | No provider fee sourced; requester pays transfer/storage | `DECISION_NEEDED` | Low provider-fee candidate but completeness/timeliness and AWS costs remain. |
| Kimchi premium Korean spot/reference/FX sources | Unestimated | `DECISION_NEEDED` | Cross-market signal source for `perps/spot`; prove official/free sources first and keep venue bindings configurable. |
| Kalshi official historical API | No paid provider price sourced | `DECISION_NEEDED` | Lower-fidelity baseline unless historical L2 source is proven. |
| Polymarket official APIs | No paid provider price sourced | `DECISION_NEEDED` | Baseline/source-of-truth API family; cap/depth limits still gate fidelity. |
| AWS storage/compute/transfer/query/logs | Unestimated | `DECISION_NEEDED` | Must be explicit reserve, not hidden residual spend. |
| Canonical S3 artifact root | Unestimated | `DECISION_NEEDED` | Raw, catalog, source-proof, and backtest artifacts share one root; model storage, request, transfer, lifecycle, and retention cost together. |
| Artifact lifecycle | Unestimated | `DECISION_NEEDED` | Retain forever, no default delete; model active/archive/deep-archive transition and restore cost. |

## Issue Payload

Title: `Plan: NT-first backtesting engine spec for flexible venue/data replay`

Accepted scope: implement an NT-native runner plan over `BacktestNode`,
`BacktestEngine`, `BacktestRunConfig`, `BacktestDataConfig`,
`BacktestVenueConfig`, and `ParquetDataCatalog`; define run manifest,
venue/provider bindings, execution model, catalog projection, result contract,
claim gates, and extension-surface matrix.

Out of scope: building the runner in this research phase, adding NT crates,
provider downloads, sample transforms, and live submit/cancel.

## Test Plan

- Manifest schema rejects hardcoded venue/provider identities in runner logic.
- The `binary option` and `perps/spot` fixtures map through the same code path.
- The resolved NT dependency can read/write/query a small multi-instrument
  `ParquetDataCatalog` fixture from configured S3 `artifact_root`, or the
  implementation records the supported staging path and blocks direct S3
  catalog claims.
- Parquet catalog input path and required NT data classes are proven for each
  selected fixture.
- Raw, catalog, source-proof, and backtest artifacts resolve under one S3
  `artifact_root` with typed subpaths.
- Artifact Index validation proves committed snapshots are reached through
  generated per-kind latest pointers, stale or hash-invalid pointers fail,
  staged/orphan events are not treated as committed truth, cross-kind parents
  are resolved through manifest lineage ids and `sha256` hashes, event/snapshot
  format choices are recorded, and normal readers do not recursively list S3 for
  discovery.
- Manifest-to-NT mapping artifact covers every manifest field and is recorded
  in this plan's future `Manifest-To-NT Mapping` section before implementation
  review.
- `BacktestResultContract` validation fails if it encodes subjective
  strategy-promotion or escalation decisions.
- One fixture records NT defaults without relying on hidden code fallback.
- One fixture passes through an NT venue or engine option.
- One fixture requests a custom-owned or unsupported surface and proves the
  configured behavior.
- Result validation rejects execution-quality claims for non-L2 data.
- Kimchi premium source proof rejects hardcoded Korean venue names and
  future-leaking reference/FX joins.
- Artifact-root validation rejects separate canonical roots and hidden local
  output fallback.
- Lifecycle validation rejects default expiration/delete rules.
- Lifecycle validation proves active-to-inactive transition follows configured
  quiet window.
- Lifecycle validation keeps current Artifact Index pointer/snapshot metadata in
  active/queryable storage.

## Manifest-To-NT Mapping

Future implementation must record the exhaustive manifest-to-NT mapping in this
section before implementation review. The mapping must cover every
`BacktestingRunManifest` obligation and its target NT config field, default,
validation rule, and unsupported-surface behavior. Exact TOML keys and nesting
are finalized only after this mapping exists.

## Residual Risks

- Current Bolt manifest may not directly enable all selected NT crates/features.
- Historical L2 replay is source-specific and not proven by live adapter support.
- Provider prices, licenses, and usage limits can drift and must be refreshed.
- Custom components can only be accepted if they preserve NT as trading truth.
