# Contract: NT-First Research Planning Package

## Authority

- `evidence.md` is the authority for claims.
- Root shared docs, numbered project docs, and issue payloads must cite or
  inherit ledger rows.
- A `GAP` cannot be described as implemented scope.
- A `USER_ASSUMPTION` can drive planning, but issue acceptance must say what proof will confirm or falsify it.
- This contract authorizes planning artifacts only. Backtesting engine, research
  analytics, dashboard UI/read-model, collectors, and runtime changes are future
  vertical project scopes.

## Source-Of-Truth Chain

```text
Raw evidence record
  -> deterministic NT catalog projection
  -> NT BacktestNode / NT reports / NT events / NT snapshots
  -> analytics read model
  -> read-only dashboard / notebooks
```

Rules:

- Raw evidence is audit input, not trading truth.
- NT catalog is canonical replay/backtest input.
- NT reports/events/snapshots are trading truth for orders, fills, positions, PnL, account state, and portfolio state.
- Analytics read models are derived and must carry source hashes/freshness.
- Dashboards are read-only and must expose staleness.
- Dashboard outlook/strategy-state fields must be backed by an accepted source
  contract or omitted/labeled as non-trading-truth.
- Dashboard must not calculate strategy state or outlook as trading truth; it
  may only display accepted runtime/analytics source fields or explicitly
  labeled exploratory output.
- Dashboard UI/tooling must pass a product-fit gate before bespoke UI work:
  Grafana for ops observability, Metabase/Preset/Superset for SQL BI, Retool
  for internal-tool workflows, and Plotly/Dash for custom visual apps. Product
  choice cannot change source truth.

## Artifact Storage Contract

Canonical raw provider/API/archive payloads, NT `ParquetDataCatalog` data,
source proof artifacts, and backtest outputs share one TOML/config-owned S3
`artifact_root`.

Typed subpaths under that root are:

- `raw/`
- `nt-catalog/`
- `source-proofs/`
- `backtests/`
- `artifact-index/`

Do not add separate canonical roots for raw data, catalog data, source proofs,
or backtest outputs. Local filesystem paths may be used only for disposable
cache or small development fixtures; they are not canonical source of truth.
No hidden cwd, temp-directory, or sibling-project fallback path is allowed.

## Artifact Index Contract

The artifact index is a thin table of contents for canonical artifacts under
`artifact_root`. It is not a custom data lake engine, warehouse, replacement for
NT `ParquetDataCatalog`, or second truth source for PnL, fills, positions,
reports, or strategy results.

Index layout under the same configured `artifact_root` is proof-gated during
Backtesting Engine implementation. The required logical pieces are:

- immutable event records
- committed snapshot artifacts
- snapshot manifests
- a generated latest-pointer object

JSON, Parquet, and a `latest.json` object name are candidate formats/names, not
final architecture decisions until the implementation proves the configured
artifact store and reader path.

Rules:

- Every canonical raw, NT catalog, source-proof, and backtest artifact has an
  artifact-local manifest in the selected structured format.
- Artifact Index write authority is producer-owned. The job that produces a
  canonical artifact writes that artifact's manifest and index event, then
  participates in the approved snapshot commit path for that artifact kind.
- Research Analytics and Dashboard are read-only consumers for upstream raw,
  NT catalog, source-proof, and backtest artifact records. A project may write
  index records only for artifact kinds it owns and produces; it must not mutate
  another project's artifact records.
- Artifact manifests and index events are structured records with `schema_version`, UTC
  `created_at`, `artifact_id`, `artifact_kind`, URI, content hash, lineage ids,
  lifecycle state, producer/owner id, and source/fidelity fields relevant to
  that artifact kind.
- Index snapshots are committed query surfaces for bulk discovery by
  Backtesting Engine, Research Analytics, and Dashboard; exact format is chosen
  after proof.
- The latest pointer is generated only. It is not manually maintained and
  is not the source of truth.
- The commit point is the conditional update of the latest pointer to a
  snapshot manifest. Events or artifacts not reachable from the snapshot
  referenced by the latest pointer are staged or orphan audit input, not
  committed discovery truth.
- Readers trust only the snapshot reachable from the latest pointer after
  verifying the snapshot hash recorded by the pointer and the snapshot manifest.
- Writers create immutable manifests, events, snapshots, and snapshot manifests
  with create-only semantics. They must not overwrite a different payload at the
  same id.
- Writers update the latest pointer with S3 conditional writes:
  `If-None-Match: *` for first creation and
  `If-Match: <previous pointer ETag>` for updates.
  If the conditional write fails, the writer must re-read latest, rebuild or
  rebase the snapshot, and retry.
- The configured artifact store must prove support for the required conditional
  write semantics before relying on this S3-native commit path. If unsupported,
  implementation must select an approved commit coordinator or table format
  before claiming reliable index commits.
- S3 ETag must not be treated as the artifact content hash. Content hashes use a
  declared hash algorithm. Multi-object artifact hashes are computed from a
  canonical sorted manifest of relative path, size, and object content hash.
- Recursive S3 listing is forbidden for normal artifact discovery. Listing is
  allowed only for off-path reconciliation, recovery, and compaction jobs that
  detect staged events, orphan bytes, or index drift.
- Current latest pointer, the referenced current snapshot, and metadata needed
  to resolve that snapshot remain in active/queryable storage. Lifecycle rules
  must not archive or deep-archive the hot index path.
- Artifact-local handles, such as a returned `BacktestResultContract`, may be
  used by the caller that just produced the artifact. Cross-run discovery uses
  the committed artifact index snapshot.

## Result And Promotion Boundary

`BacktestResultContract` is an objective evidence and lookup contract. It may
carry NT result/report pointers, metrics artifact pointers, source proof ids,
catalog/source hashes, strategy config hash, run purpose, fidelity class, claim
limits, warnings, and mechanical blockers.

It must not carry a subjective promotion recommendation such as "use this
strategy" or "escalate this strategy." Strategy review status belongs to a
Research Analytics `PromotionPackage` or a later explicitly owned review
artifact that consumes one or more backtest result contracts as evidence.

## Source Proof Contract

`SourceProofReport` is a Bolt-owned thin evidence gate. It is not an NT object,
provider adapter, data warehouse, or replacement for `ParquetDataCatalog`.

No raw/provider/archive data may become canonical NT catalog input or backtest
input until an accepted `SourceProofReport` exists for that fixture/source
binding.

Acceptance authority:

- Backtesting Engine/source-proof implementation owns acceptance for source
  proofs used as canonical NT catalog input or backtest input.
- Automated acceptance is allowed from the first implementation, not deferred
  to a later phase, but only when every required schema, sample, license,
  time/freshness, NT mapping, fidelity, and forbidden-claim check passes. Any
  missing, ambiguous, expired, unsupported, or contradictory proof must remain
  `pending` or become `rejected`; automation must not silently downgrade a
  failed required check into acceptance. Manual review is a fallback or
  override path, not the only intended acceptance path.
- Research Analytics and Dashboard consume accepted reports read-only. They may
  reject or narrow their own downstream use, but they must not mark upstream
  source proofs accepted, upgrade fidelity, or weaken forbidden claims.
- Accepted `SourceProofReport` records are immutable. New schema, license,
  sample, fidelity, mapping, or claim-limit facts create a new proof version
  that supersedes the old one; they must not rewrite the accepted record.
- Backtest results that used a now-superseded accepted proof remain valid as
  historical artifacts tied to their original manifest and proof version. New
  backtest runs must use the latest accepted proof for the source binding unless
  the run manifest explicitly pins an older accepted proof version for
  reproducibility. A non-latest proof pin must include
  `proof_pin_reason_code`; latest-proof runs do not need pin-reason fields.
  `proof_pin_reason_detail` is required for `audit_or_investigation` and
  optional for other codes.

Run purpose:

- `run_purpose = "normal" | "reproduction" | "audit" | "regression" | "migration"`
- `normal` runs must use the latest accepted proof and cannot pin older proof.
- Non-latest proof pins are allowed only for `reproduction`, `audit`,
  `regression`, or `migration` runs with the matching structured reason fields.

Allowed `proof_pin_reason_code` values:

- `baseline_reproduction`
- `published_result_reproduction`
- `regression_comparison`
- `audit_or_investigation`
- `migration_validation`

Required fields:

- `source_proof_id`
- `source_proof_version`
- `status = "pending" | "accepted" | "rejected"`
- `supersedes_source_proof_id`, if this proof replaces an earlier accepted
  report
- `latest_for_source_binding`, or an implementation-equivalent pointer/index
  field identifying the latest accepted proof for a source binding
- `proof_pin_reason_code` in the run manifest when using a non-latest accepted
  proof
- `proof_pin_reason_detail` when `proof_pin_reason_code =
  "audit_or_investigation"`; optional for other allowed codes
- `run_purpose` in the run manifest; `normal` cannot use non-latest proof
- `accepted_by` and `accepted_at` when status is `accepted`
- `acceptance_mode = "automated" | "manual"`
- required-check results for schema, sample, license, time/freshness, NT
  mapping, fidelity, and forbidden claims
- market-structure fixture: `binary option` or `perps/spot`
- TOML/registry source binding key, not hardcoded venue logic
- source family, provider/API/archive pointer, and access method
- instrument/market coverage
- source time range, capture time, event-time and availability-time semantics
- schema version, field list, sample pointer, and sample/content hash
- historical order-book snapshot/delta availability when claiming `L2_REPLAY`
- license/commercial-use boundary and proof timestamp
- NT catalog/data-class mapping status or approved signal-input status
- fidelity class and forbidden claims
- warning/gap list
- artifact_root URI for source proof artifacts and samples

The report stores pointers, hashes, classifications, and claim limits. It must
not duplicate heavy raw data, catalog data, or result payloads.

## Artifact Lifecycle Contract

Canonical artifacts default to retained forever. No default lifecycle rule may
delete or expire canonical artifacts.

Archive storage under `$5/month` is treated as zero for planning decisions.
This does not make restore, request, minimum-duration, metadata, or retrieval
costs disappear; those remain explicit cost fields.

Required lifecycle fields or tags:

- `retention = "forever"`
- `storage_profile = "active" | "archive" | "deep_archive"`
- `artifact_kind = "raw" | "nt_catalog" | "source_proof" | "backtest" | "artifact_index"`
- `lifecycle_state = "active" | "inactive"`
- `quiet_window`
- `project`, plus run/source/dataset id when available.

Storage profiles:

- `active`: keep in the active S3 class used by the implementation branch.
- `archive`: transition to a colder archive class after the configured active
  window, then optionally to deep archive after the configured archive window.
- `deep_archive`: transition to the coldest approved S3 archive class when the
  artifact is not expected to be read except for audit/restore.

Transition windows are TOML/config-owned. Scratch or failed artifacts may be
tagged separately for faster archive transition, but still must not be deleted
by default. Any purge/delete policy requires a separate explicit approval.

Lifecycle state rule:

- Every canonical artifact starts as `active`.
- After its configured `quiet_window` passes, it becomes `inactive`.
- `inactive` means eligible for archive/deep-archive transition, not deletion.
- Archive and deep-archive placement are `storage_profile` changes; they do not
  introduce additional lifecycle states.
- Future implementation sessions must define the exact quiet-window values and
  timestamp basis for each artifact kind.

## Backtesting Extension Surface Contract

- The future Backtesting Engine is NT-first, not NT-default-only.
- Every relevant NT/custom surface must be classified before implementation as
  `defaulted`, `pass_through`, `custom_owned`, or `unsupported_for_now`.
- The classification must cover at minimum backtest engine config, venue
  simulation config, run config, catalog storage/protocol options, strategy
  selection, actor/execution-algorithm selection, risk, portfolio, execution,
  cache, message bus, streaming, fill, fee, latency, margin, leverage, queue,
  liquidity, settlement, and order-behavior surfaces.
- `defaulted` surfaces must write the resolved NT/default value into the run
  manifest and result claim limits.
- `pass_through` surfaces must map from TOML/manifest to the NT config field
  without venue/provider-specific branches.
- `custom_owned` surfaces must prove an NT-compatible interface and must not
  create independent execution, PnL, position, fill, account, or portfolio truth
  unless explicitly labeled exploratory/non-trading-truth.
- `unsupported_for_now` surfaces must fail fast if requested.
- Contract fixtures must include at least one `defaulted`, one `pass_through`,
  and one `custom_owned` or `unsupported_for_now` surface so the runner cannot
  pass review through a single happy path.

## Venue Gates

Venue/product/provider identity is selected through TOML-backed registry or
binding entries. The core runtime, admission path, secret path, future
Backtesting Engine orchestration, Research Analytics projection, and Dashboard
must not branch on hardcoded venue names.

Backtesting proof fixtures are named by market structure: `binary option` and
`perps/spot`. Venue/provider names in this table are evidence examples or
candidate bindings only; they are not required first fixtures, module names, or
architecture branches.

| Venue/surface | Planning stance | Required proof before implementation claim |
|---|---|---|
| Hyperliquid HIP-4 | Upstream NT-supported on `develop`; target `bolt-v2` branch must select an NT version with required support. | Compile/API proof plus lifecycle matrix for instruments, orders, fills, settlement, reconciliation, and historical-data class. |
| Kalshi | Adapter support assumed by user instruction. | Data-source, historical-fidelity, order/fill/report, and backtest claim-limit proof. Do not turn adapter invention into this scope. |
| Perpetual futures venues | Use NT live adapters only for source-proven venue/product surfaces; Tardis, OKX official data, Hyperliquid archive, Kaiko, CoinAPI, Amberdata, or other official/vendor sources are candidates for historical replay. Checked venue examples are evidence instances, not special architecture paths. | Fidelity/license/schema/sample proof and replay-to-catalog contract before selecting a provider; official venue capture needs official-source proof before use. |
| Polymarket | Use NT adapter/loader first. | Official API cap/depth proof before adding Telonex, MarketLens, PMXT, PolyBackTest, PolymarketData, Goldsky, or other supplement. |
| Dashboard/PnL | NT reports/events/snapshots first. | #409 or equivalent `PortfolioSnapshot` capture proof, #77 durable trade-history/PnL path, and #36 inclusion/exclusion decision for redemption realized PnL before dashboard PnL completeness claims. |
| Dashboard/BI product | Existing products before custom UI. | Source-contract, security, query-backend, UX, and all-in monthly cost proof before selecting Grafana, Metabase, Preset/Superset, Retool, Plotly/Dash, or bespoke UI. |

## Provider Gates

- Cost is modeled before implementation and used for user review/cut decisions,
  not for premature weak architecture.
- Tardis, Kaiko, CoinAPI, Amberdata, official archives, and similar providers
  need all-in subscription/storage/compute/transfer estimates.
- Telonex, MarketLens, PMXT, PolyBackTest, PolymarketData, and Goldsky need
  license, schema, retention, freshness, and sample-data proof before selection.
- Goldsky/on-chain sources are provenance supplements unless paired with CLOB
  orderbook source.
- Official archives/APIs must be labeled by freshness and completeness.
- Official API/archive capture is a `GAP` per venue until source-proven.
- Forward capture cannot backfill historical L2 claims.
- Final provider selection must refresh price, license, and usage-limit evidence
  at selection time; planning-snapshot prices are not final acceptance evidence.

## Binding Contract Tests

- Venue/provider swaps must be represented by TOML and registry/binding data
  changes only.
- Contract tests must fail if core runtime, admission, secret resolution,
  Backtesting Engine orchestration, catalog projection, analytics read model, or
  dashboard code branches on concrete venue or provider names.
- The same test fixture must exercise at least two venue/provider bindings so a
  single hardcoded happy path cannot satisfy the gate.
- Backtesting extension-surface contract tests must fail when NT defaults,
  custom-owned behavior, or unsupported surfaces are omitted from the run
  manifest.

## Prohibited Claims

- Do not treat Kalshi adapter feasibility as a blocker in this package; adapter
  readiness is a user assumption, while data/fidelity/source-contract proof
  remains required.
- Do not claim HIP-4 historical execution-quality backtesting from live adapter evidence alone.
- Do not claim any provider is selected before fidelity, license, schema/sample,
  and cost-impact evidence are recorded.
- Do not claim dashboard PnL completeness while `PortfolioSnapshot` remains uncaptured.
- Do not create a Bolt backtest engine, executable order schema, or venue translation layer without explicit evidence that NT cannot provide the needed surface.
- Do not hardcode concrete venue, product, provider, market, account, or credential identity into core runtime, research, or dashboard logic.

## Existing Issue Map

| Issue | Relation |
|---|---|
| #19 | Existing data lake lineage metadata; link raw evidence/catalog lineage rather than duplicating it. |
| #20 | Existing canonical normalized lake layout work; do not redefine the lake layout here. |
| #21 | Existing normalized resolutions with provenance work; analytics may consume later. |
| #22 | Existing versioned normalized markets dimension work; provider gates should not replace it. |
| #23 | Existing instrument spool bridge; dependency for complete reduced ETL seam. |
| #24 | Existing NT-first data lake follow-on epic; do not duplicate its ETL/lake scope. |
| #34 | Existing Polymarket strategy platform epic; dashboard/research should not silently expand it. |
| #36 | Existing redemption-realized-PnL/history issue; dashboard must link or explicitly exclude redemption scope. |
| #39 | Existing adaptive venue weighting issue; research analytics may feed it later, but should not make it baseline scope. |
| #75 | Existing offline verified allowlist/research participation workflow; keep research workflow links explicit. |
| #77 | Existing durable trade-history/PnL path issue; dashboard historical PnL depends on it or must label the gap. |
| #88 | Existing deferred strategy/PnL reconciliation context; dashboard must not claim closure. |
| #112 | Existing Kalshi venue epic; new Kalshi proof slice should update or depend on it. |
| #115 | Existing HIP-4 issue with premise stale relative to upstream NT `develop`; update/link and prove target `bolt-v2` branch support rather than duplicate or imply closure. |
| #127 | Existing native Polymarket order book depth issue; Polymarket depth/fidelity proof may depend on it or must label the gap. |
| #148 | Existing inline-capture sidecar risk issue; comprehensive capture must respect its deferred trigger stance. |
| #176 | Existing agent-readiness/autonomy roadmap; tooling context, not implementation scope. |
| #236 | Thin NT rebuild epic; architecture parent for NT-first/no-dual-path constraints. |
| #254 | Existing Polymarket V2 adoption blocker issue; Polymarket source readiness must link or explicitly exclude it. |
| #385 | Existing live connectivity proof issue; live connectivity proof is not historical backtest proof. |
| #407 | Existing controlled Polymarket broad-discovery mode issue; discovery breadth constraints must link rather than duplicate it. |
| #409 | Existing `PortfolioSnapshot` capture issue; dashboard PnL completeness depends on it. |

## Review Contract

- Review the ledger first, then the prose artifacts.
- Findings should cite exact ledger row, source line, or missing proof.
- External review should challenge evidence classification, not seek consensus over prose.
- No issue creation or mutation without explicit user approval.

## Future Vertical Contracts

Each future project must have its own spec/plan/tasks/issues. This package may
stage them but must not merge them into one implementation project.

| Future project | Project directory | Must prove before implementation |
|---|---|
| Backtesting Engine | `1-backtesting-engine/` | NT crate/feature availability, manifest-to-NT config mapping, extension-surface classification, resolved default recording, catalog data classes, fill/fee model ownership, result truth, fidelity labels, and two market-structure fixtures with TOML/registry venue/provider bindings. |
| Research Analytics | `2-research-analytics/` | Raw evidence schema, deterministic projection lineage, point-in-time correctness, experiment metadata, notebook boundary, claim gates, and promotion to typed TOML/NT-compatible runtime contract. |
| Dashboard | `3-dashboard/` | Field-by-field source matrix, freshness/staleness rules, no-mutation controls, product selection gate, #409/#77/#36 disposition, and #369 non-closure context. |
