# Open Questions And Review Prompts

This is the single root handoff for unresolved questions in the
`023-nt-research-analytics-platform` planning package.

Use it after reading `README.md`. Do not implement from this document. Pick one
question, answer it with evidence, and then update the relevant project
`spec.md`, `plan.md`, or `tasks.md`.

## Global Context

- The three verticals remain separate projects:
  `1-backtesting-engine`, `2-research-analytics`, and `3-dashboard`.
- Default implementation order is Backtesting Engine first. Research Analytics
  consumes backtest/source-proof/result contracts. Dashboard displays accepted
  read models and artifact links from the other two verticals.
- Venue/provider names are examples only. Design by market structure and
  configurable source bindings, not by hardcoded venue branches.
- Backtesting fixtures are `binary option` and `perps/spot`.
- Cost is a review lever, not an architecture limiter. Pick the best-fidelity
  architecture first, then model cost and cut levers.
- Canonical artifacts live under one TOML/config-owned S3 `artifact_root`, with
  typed subpaths. Local paths are only cache/development fixtures.
- Source proof records are immutable. A changed proof supersedes the old proof
  with a new version. Historical backtests stay valid against the proof version
  they used.
- Normal backtests use the latest accepted proof by default. Older accepted
  proof versions are allowed only when a manifest explicitly pins them for a
  non-normal purpose such as reproduction, audit, regression, or migration.
- The Artifact Index is intended to be a thin table-of-contents layer, not a
  warehouse or replacement for NT catalog truth. JSON, Parquet, and
  `latest.json` are candidate implementation choices, not final decisions.
- No GitHub issue mutation or external review request is authorized by this
  document.

For every prompt below, require the responder to return:

1. Evidence inspected, with file/path/URL references.
2. Recommendation.
3. Explicit rejected alternatives and why.
4. Required doc changes.
5. Tests or proof gates needed before implementation.

## Backtesting Engine

### OQ-001: Artifact Path Convention And Instrument Identity

Question: What is the canonical path/key convention under the shared S3
`artifact_root` for raw payloads, NT catalog data, source proofs, backtest
outputs, and artifact-index objects when there may be hundreds or thousands of
instruments?

Context: Current docs require one S3 `artifact_root` and typed subpaths
`raw/`, `nt-catalog/`, `source-proofs/`, `backtests/`, and `artifact-index/`.
The path design must support binary-option markets, perps, spot, and
cross-market signals such as kimchi premium. It must not encode a fixed venue
set, must not create infinitely long directory names, and must not require
recursive S3 listing for normal discovery.

Prompt:

> Review the current `1-backtesting-engine` docs and the shared artifact/index
> constraints. Research NT catalog path conventions and relevant data-provider
> storage conventions, including but not limited to Nautilus Trader,
> Databento, Tardis, cryptofeed, and public exchange/archive datasets. Recommend
> a canonical S3 path/key convention for:
> raw provider/API/archive payloads, NT `ParquetDataCatalog` data,
> source-proof samples/reports, backtest outputs, and artifact-index records.
> The recommendation must support hundreds or thousands of instruments without
> overlong path segments, preserve configurable venue/product/provider identity,
> support `binary option` and `perps/spot` fixtures, and handle cross-market
> signals such as kimchi premium without hardcoding Korean venues. Provide
> concrete examples for one binary-option market, one perp, one spot market,
> and one kimchi-premium source family. State which identifiers belong in the
> path, which belong in manifests/index metadata, and which must remain
> registry/config-selected.

### OQ-002: Artifact Index Backend, Format, And Commit Rule

Question: What exact index format/backend and commit rule should implement the
thin Artifact Index?

Context: Current docs intentionally do not choose JSON, Parquet, SQLite,
DuckDB, table formats, or a `latest.json` object name. The desired behavior is
an append/commit-friendly table of contents: immutable events or equivalent
records, committed snapshots, generated latest pointer, content hashes,
producer-owned writes, read-only consumers, staged/orphan recovery, and no
recursive S3 discovery on the normal read path.

Prompt:

> Design the exact Artifact Index implementation for the Backtesting Engine
> planning package. Compare flat JSON manifests, Parquet manifests, SQLite or
> DuckDB metadata, Iceberg/Delta-style table metadata, and S3 pointer objects.
> Decide whether the index should be global, per artifact kind, per import/run,
> or a hybrid with rollups. Define the commit point and how the latest pointer
> is updated safely. Prove whether S3 conditional writes are sufficient for the
> configured artifact store; if not, recommend the smallest commit coordinator
> or table format. Keep the layer thin: it must locate artifacts and validate
> hashes, not become a warehouse. Include failure handling for stale pointers,
> hash mismatch, concurrent writers, staged/orphan records, and reader
> currentness.

### OQ-003: NT `ParquetDataCatalog` S3 Proof

Question: Can the NT version resolved by the target `bolt-v2` branch directly
read, write, and query `ParquetDataCatalog` data from the configured S3
`artifact_root`?

Context: Docs say to use the NT version resolved by the respective target
`bolt-v2` branch, not a fixed SHA. Direct S3 access is allowed only after proof
of the required NT crates/features/storage options. If direct S3 is not
supported, a supported staging path must be documented before implementation.

Prompt:

> Inspect the NT dependency resolved by the target `bolt-v2` branch and prove
> the actual `ParquetDataCatalog` input/output path for a small multi-instrument
> fixture. Identify required crates, feature flags, storage options, URI
> support, and any limitations. Run or specify the minimal proof needed to show
> read/write/query behavior from S3. If direct S3 catalog access is unsupported
> or uncertain, define the supported staging path and the implementation gate
> that prevents hidden local fallback from becoming canonical.

### OQ-004: `SourceProofReport` Schema And Automated Acceptance

Question: What exact `SourceProofReport` schema and automated acceptance rule
should gate source/provider selection?

Context: Source proof must be robust for schema, license, sample, fidelity,
time availability, NT mapping, forbidden claims, cost, and status. Automation
is allowed from the initial implementation if every required check is
deterministic and fail-closed. Manual review is a fallback or override, not a
reason to defer automation indefinitely.

Prompt:

> Define the exact `SourceProofReport` schema, required checks, statuses,
> immutable versioning, supersession fields, acceptance authority, and automated
> acceptance rule. Include checks for source availability, schema, representative
> sample pointer/hash, license and personal/commercial boundary, historical
> order-book snapshot/delta availability where relevant, retention/freshness,
> event-time/availability-time correctness, NT data-class mapping, fidelity
> class, forbidden claims, and cost. Specify which checks can be automated
> immediately, which require reviewer override, and how ambiguous/missing/failed
> checks block acceptance. Define tests that reject mutation of accepted proof
> records and reject automated acceptance when any required check is absent.

### OQ-005: Provider Proof Order For `binary option` And `perps/spot`

Question: What exact source-proof order should be applied to the two market
structure fixtures before any provider is selected?

Context: The direction is to prove official/free source candidates first, then
paid/vendor candidates if fidelity is insufficient, then mark
forward-capture-pending if no usable historical source exists. The fixtures are
market-structure based, not venue based: `binary option` and `perps/spot`.
Kimchi premium belongs under `perps/spot` as a cross-market signal family.

Prompt:

> Produce an evidence-based source-proof order for the `binary option` fixture
> and the `perps/spot` fixture. Do not choose a final provider. For each
> fixture, list official/free source classes to inspect first, paid/vendor
> source classes to inspect only after insufficient fidelity is proven, and the
> condition that moves the fixture to `FORWARD_CAPTURE_PENDING`. Include
> kimchi-premium sources as configurable Korean spot/reference/FX inputs under
> `perps/spot`. Make clear that Polymarket, Kalshi, Hyperliquid, Upbit,
> Bithumb, Binance, Bybit, OKX, Tardis, Kaiko, CoinAPI, Amberdata, Telonex,
> PMXT, MarketLens, PolyBackTest, and PolymarketData are examples or candidate
> bindings only, never architecture assumptions.

### OQ-006: L2 Order Book And Fidelity Claim Rules

Question: What exact evidence is required before a result may claim
execution-quality replay?

Context: We may use L2 order-book data when available and proven. We may also
allow weaker data, but weaker data must never be represented as proving
execution-quality behavior. The current labels are `L2_REPLAY`,
`TRADE_BAR_REPLAY`, `SIGNAL_ONLY`, and `FORWARD_CAPTURE_PENDING`.

Prompt:

> Define the exact criteria for each fidelity class and the forbidden claims
> attached to each class. Ground the criteria in NT backtesting mechanics and
> data-source capabilities, not intuition. Specify what historical order-book
> depth, snapshot/delta sequence, trade/bar data, event timing, liquidity,
> queue-position, fee, latency, and settlement evidence is required for
> `L2_REPLAY`. Specify what weaker data can support and the exact wording that
> prevents execution-quality claims. Include how these rules apply to
> binary-option settlement and perps/spot continuous markets.

### OQ-007: Manifest TOML Schema And Manifest-To-NT Mapping

Question: What exact TOML schema should `BacktestingRunManifest` use after all
manifest obligations map to NT configuration?

Context: Obligations are already defined at a contract level. Exact TOML should
be finalized during Backtesting Engine implementation after mapping to NT
`BacktestRunConfig`, `BacktestDataConfig`, `BacktestVenueConfig`, and related
configuration surfaces. It must remain venue/provider agnostic and must support
latest accepted source proof by default plus explicit older-proof pins for
non-normal run purposes.

Prompt:

> Finalize the `BacktestingRunManifest` TOML schema by mapping every manifest
> obligation to the resolved NT configuration surfaces. Include run id, target
> `bolt-v2` branch/ref, NT version resolved by that branch, source proof id and
> version, run purpose, proof pin reason code/detail, strategy config hash,
> catalog path/hash, manifest hash, execution model, fixture type, fidelity
> class, claim limits, output prefix, warnings/gaps, and deferred currentness
> rules. Produce an exhaustive manifest-to-NT mapping artifact and tests that
> fail when any required manifest field is unmapped.

### OQ-008: Strategy Execution Wiring

Question: What does the Backtesting Engine accept as a strategy input, and how
does it wire that strategy into NT?

Context: Backtesting Engine owns running strategies in NT. Valid strategy
inputs are existing compiled Rust strategies, human-written typed config, and
future Research Analytics promotion packages that generate typed config. Inline
strategy code, notebook runtime code, Python strategy paths, and untracked
config blobs are rejected.

Prompt:

> Define the strategy execution wiring contract for Backtesting Engine. Identify
> how an existing compiled Rust strategy is selected, how human-written typed
> config is validated, and how a future RA-generated typed config artifact is
> accepted. Map the strategy selection and config into NT strategy/actor or
> execution-algorithm surfaces. Define validation rules for hashes, config
> provenance, artifact references, forbidden inline code, notebook/Python paths,
> and runtime mutation. Include tests that prove two different TOML-selected
> strategy/config bindings use the same code path.

### OQ-009: `BacktestResultContract` Exact Field Schema

Question: What exact result contract should Backtesting Engine emit?

Context: The contract must be objective evidence only. It must not recommend
whether to use, escalate, approve, or promote a strategy. Research Analytics
owns review/promotion status. Dashboard may display RA-owned status later.

Prompt:

> Finalize the exact `BacktestResultContract` schema after inspecting selected
> NT output shapes. Include objective trace fields such as run id, target
> `bolt-v2` branch/ref, resolved NT version, manifest hash, strategy config
> hash, source proof id/version, catalog path/hash, fixture type, fidelity
> class, claim limits, NT report/result pointers, artifact URIs, warnings/gaps,
> mechanical blockers, and created_at. Explicitly exclude subjective promotion
> labels or recommendations. Define validation and consumer examples for
> Research Analytics and Dashboard.

### OQ-010: NT Extension Surface Policy

Question: Which NT surfaces use defaults, pass-through config, custom-owned
logic, or unsupported-for-now behavior?

Context: The design should not blindly accept NT defaults or hardcode policy.
It should start thin but remain able to use NT `BacktestEngineConfig`,
venue-simulation controls, risk/portfolio/execution/cache/msgbus/streaming
config, fill/fee/latency/margin/leverage/queue/liquidity/settlement behavior,
actors, strategies, and execution algorithms where proof requires them.

Prompt:

> Build the `BacktestExtensionSurface` matrix for the resolved NT version.
> Classify each surface as `defaulted`, `pass_through`, `custom_owned`, or
> `unsupported_for_now`, and explain what evidence would change the
> classification. Include engine, venue simulation, run config, catalog,
> strategy, actor/execution algorithm, risk, portfolio, execution, cache,
> message bus, streaming, fill, fee, latency, margin, leverage, queue,
> liquidity, settlement, and order-behavior surfaces. Require resolved defaults
> to be materialized into the manifest/result evidence so future readers know
> which policy was actually used.

## Research Analytics

### OQ-011: Research Analytics Contract After BTE Result Contract

Question: What should Research Analytics define now, and what should wait until
the exact `BacktestResultContract` exists?

Context: Research Analytics should be implementation-ready at contract level,
but should not overdesign internal schemas before BTE finalizes exact result
and source-proof contracts. RA consumes objective BTE outputs and source proof
metadata; it does not run backtests or replace source truth.

Prompt:

> Review `2-research-analytics` against the current BTE contract state. Identify
> which RA schemas and tasks are ready now and which should remain interface
> placeholders until BTE finalizes `BacktestResultContract`,
> `SourceProofReport`, and Artifact Index schemas. Preserve implementation
> readiness at contract level without inventing internal storage models too
> early. Return specific doc changes that reduce overdesign while keeping
> acceptance criteria testable.

### OQ-012: RA-Owned Derived Artifact Kinds And Schemas

Question: Which derived artifacts may Research Analytics write, and what schema
must each artifact kind use?

Context: RA may produce derived artifacts only after explicit artifact
kind/schema is defined. RA must not mutate raw payloads, NT catalog artifacts,
BTE source proofs, BTE result contracts, or upstream artifact-index truth.
Generated promotion/config artifacts should live under the shared S3
`artifact_root` in an RA-owned artifact family.

Prompt:

> Define the first allowed RA-owned derived artifact kinds and schemas. Include
> research datasets, feature tables, experiment result summaries, leakage-check
> reports, promotion packages, and generated typed config artifacts only if each
> kind has an explicit owner, schema, source references, content hash, lifecycle
> status, and Artifact Index behavior. Define which artifacts are immutable,
> which may be superseded, and which can be consumed by Dashboard. Add tests
> that reject RA mutation of upstream BTE/source artifacts and reject derived
> artifacts without an explicit kind/schema.

### OQ-013: `PromotionPackage` Schema And Status Workflow

Question: What exact `PromotionPackage` schema and review workflow should RA
own?

Context: BTE result contracts do not say whether a strategy should be used. RA
owns review/promotion status. Current status labels are `draft`, `blocked`,
`ready_for_review`, `changes_requested`, `rejected`, and
`approved_for_config`. `changes_requested` is the iterate/research-more state.
`approved_for_config` only permits typed TOML/NT-compatible config output for
later implementation/review; it is not live-trading approval.

Prompt:

> Finalize the `PromotionPackage` schema and status transition rules. Include
> required BTE evidence references, source proof ids/versions, claim limits,
> fidelity compatibility, objective result refs, reviewer/policy refs,
> dashboard-facing fields, typed config artifact refs, rejection/change reasons,
> and non-live boundary fields. Define allowed transitions among `draft`,
> `blocked`, `ready_for_review`, `changes_requested`, `rejected`, and
> `approved_for_config`. Prove that no state auto-merges, auto-enables,
> schedules live trading, touches SSM, or mutates production runtime config.

### OQ-014: Point-In-Time And Leakage Rules For Research Features

Question: What exact point-in-time rules prevent feature leakage in RA?

Context: RA may join source proof metadata, catalog projections, backtest
results, market data, and derived features. It must preserve event time,
availability time, source proof version, claim limits, and artifact hashes.

Prompt:

> Define the exact point-in-time and leakage-prevention rules for Research
> Analytics. Cover event time vs availability time, cross-market joins such as
> kimchi premium, source proof versioning, artifact hashes, backtest result
> timestamps, dashboard-facing derived fields, and notebook read-only access.
> Specify test fixtures that intentionally leak future data and must fail.

## Dashboard

### OQ-015: Field-Source Matrix And Read-Model Shape

Question: What exact dashboard fields exist, and what read model/query shape
serves them?

Context: Product selection should not happen first. Define the field-source
matrix and query/read-model shape before choosing Grafana, Metabase,
Superset/Preset, Retool, Plotly/Dash, or custom UI. Dashboard is read-only and
must not compute independent PnL/account/exposure/MTM truth.

Prompt:

> Build the exact Dashboard field-source matrix and read-model shape before
> product selection. Cover orders, fills, positions, account state, portfolio
> equity, exposure, historical PnL, redemption-realized PnL, data health,
> source proof metadata, artifact lifecycle status, strategy-review/promotion
> status, and outlook/strategy-state fields. For each field, specify source
> type, source ref, freshness rule, stale behavior, gap label, truth status,
> query/read-model shape, and whether the field is omitted, partial,
> unavailable, excluded, exploratory, authoritative, or derived. Include #409,
> #77, #36, and #369 dependency handling.

### OQ-016: Dashboard Product Gate Including Retool

Question: Which dashboard product path should be selected after the
field-source matrix and read model are known?

Context: Retool is a candidate alongside Grafana, Metabase, Superset/Preset,
Plotly/Dash, and custom UI. Product choice must not change source truth, add
mutation authority, or create a second artifact store.

Prompt:

> After the field-source matrix and read-model shape are defined, run the
> dashboard product gate. Evaluate Grafana, Metabase, Superset/Preset, Retool,
> Plotly/Dash, and custom UI against source-contract fit, query/API backend,
> no-mutation enforcement, permissions/audit logging, security, embedding or
> internal-tool needs, UX, cost, operations burden, and artifact-link behavior.
> Refresh current pricing and security claims from official sources. Recommend
> one product path or a staged product path, and explain what would force custom
> UI.

### OQ-017: Strategy State And Outlook Display Contract

Question: What can Dashboard display for strategy state and outlook without
becoming a second strategy authority?

Context: Dashboard may display accepted runtime/analytics source fields or
explicitly labeled exploratory output. It must not calculate strategy state or
outlook as trading truth, infer promotion from backtest metrics, or mutate RA
promotion state.

Prompt:

> Define the dashboard display contract for strategy state and outlook. Identify
> accepted source types, required source contracts, exploratory/non-trading-truth
> labels, freshness rules, and omission behavior when no source is accepted.
> Prove that dashboard does not calculate strategy state/outlook as trading
> truth, infer strategy approval from BTE metrics, or mutate RA-owned
> `PromotionPackage` state. Include tests and example rows in the field-source
> matrix.

### OQ-018: Dashboard Missing-Data And Gap Label Semantics

Question: What exact labels and user-facing semantics should dashboard use when
sources are missing, stale, excluded, or partial?

Context: Dashboard should not hide missing truth or invent it. If PnL,
exposure, account state, or outlook source proof is absent, the field should be
omitted or explicitly labeled.

Prompt:

> Define dashboard gap labels and semantics for `stale`, `partial`,
> `unavailable`, `excluded`, `exploratory`, and any other required state. For
> each label, define when it is used, what field types it can apply to, what the
> dashboard may still show, and which claims are forbidden. Include examples for
> missing `PortfolioSnapshot`, unresolved redemption PnL, stale source proof,
> non-latest proof pin, and exploratory strategy outlook.

## Cross-Project Process Handoff

These items are PR-cycle/process handoff questions, not design blockers for the
three implementation verticals.

### OQ-019: Gemini Code Assist Comment Resolution

Question: Which Gemini Code Assist comments are resolved by the current docs,
and which require a reply or doc change?

Context: PR #435 has Gemini Code Assist comments about dashboard
outlook/strategy-state authority, cost reserve assumptions, issue payload cost
model wording, and obsolete prototype-probe paths. These must be replied to or
disproved with current file evidence before final handoff.

Prompt:

> Review all Gemini Code Assist comments on PR #435 against the current working
> tree. For each comment, classify as fixed, disproved/obsolete, or still open.
> Provide exact file/path evidence and the reply text that should be posted. Do
> not mutate GitHub unless explicitly authorized. If a doc change is needed,
> identify the smallest project-local or shared doc patch.

### OQ-020: Final External Review Packet

Question: What exact packet should be sent for adversarial review after the
final handoff is prepared?

Context: External review should happen after consolidation, not before. Desired
reviewers are Claude, Gemini, Kimi, GLM, and DeepSeek. For DeepSeek and GLM,
run direct API reviewer doctor first and skip if unavailable. Claude must use
subscription/OAuth, not API.

Prompt:

> Prepare the final adversarial-review packet for the current worktree and
> `specs/023-nt-research-analytics-platform/`. Include exact worktree path,
> current branch/head, files to read, review goals, known decisions, and areas
> where dissent is desired. Ask reviewers to verify: three-project MECE
> separation, no venue/provider hardcoding, BTE-first dependency direction, S3
> artifact root and thin index assumptions, source-proof automation rules,
> RA-owned promotion boundary, dashboard product gate including Retool, and
> Gemini Code Assist comment disposition. Require findings with severity,
> file/path evidence, and concrete fixes.
