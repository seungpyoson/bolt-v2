# Historical Data Acquisition Architecture v1

Status: owner-approved architecture; implementation is split into the issue-owned slices below.

## Provenance

- Repository evidence base: clean `37e619b3fbd65fc041a05399ecf1750b8999567a`.
- NautilusTrader evidence base: pinned revision `d636f17604cdbddc28ad40e0e15720e2d19bf860`.
- External adversarial reviewer: Claude Code CLI, model `claude-fable-5`.
- Final external disposition: the architecture is sound after two text-level amendments: cache verification on first use per run and explicit Binance policy exclusion.
- Owner decision: proceed without another external review after incorporating those amendments.
- A subsequent internal adversarial review required legacy-authority and issue-ownership reconciliation; both are incorporated here without changing the approved design.
- Parent architecture scope: GitHub issue #437. Each runtime change remains a separately named issue or explicit slice of #437.

## Decision And Invariant

Bolt extends the existing source-neutral acquisition, operator, catalog-projection, and NT backtest path. It does not build a second ingestion platform or a second backtest engine.

The invariant is: every backtest consumes an explicit, immutable, content- and version-bound set of source-proven bytes through one NT-backed replay path, while acquisition and replay remain inside configured cost and resource limits.

The selected read design is manifest-bound, run-scoped local hydration. A future manifest-aware remote NT reader is an optimization only if the scale pilot proves local hydration is the bottleneck. Validating a remote prefix and then allowing NT to list that prefix independently is rejected because it retains a time-of-check/time-of-use race.

## Current State

The repository already has reusable source-neutral contracts, source bindings, normalized-table dispatch, NT catalog projection, an S3 artifact store, and a content-addressed source-object cache. That is a foundation, not an end-to-end comprehensive pipeline.

At the pinned revision and repository head:

- the operational source-universe batch path is trade-oriented and does not execute the full generic operator plan;
- raw funding, mark, and index acquisition normalizers remain fail-loud seams;
- OI has no safe unit/availability contract and cannot enter `BacktestNode` through a typed custom-data config;
- two or more NT data configs are materialized into one `Vec<Data>` and sorted as a whole;
- Bolt can pass a raw catalog URI to NT without binding reads to a publication manifest; and
- the existing catalog plan and evidence matrix overstate some source coverage and are not implementation authority after this decision.

## Source Selection And Coverage

Official and free sources are preferred. Tardis, Databento, and other paid historical-data products are excluded unless a later owner decision approves them after measured free-source total cost and coverage are known.

There is exactly one TOML-selected implementation for each `(venue, product_family, table_family, [start, end))` cell. There is no automatic fallback. Windows are half-open UTC intervals, sorted and non-overlapping, and their union must cover the declared window or carry an explicit measured-gap state.

Coverage authority is a machine-readable full cross-product of one approved venue/product registry and one global table-family enum. Every cell occurs exactly once. Unsupported, not-applicable, forward-only, pending-proof, and policy-excluded cells remain explicit and carry reason and evidence; omission is invalid.

Binance is not a technical fallback and is not silently omitted. Every Binance product/table-family coverage cell is `excluded_by_policy` with this exact reason:

> owner choice: Binance is intentionally excluded despite breadth loss

This is an owner-directed requirements choice. It deliberately gives up the richest free source currently represented in the legacy matrix. Bybit remains eligible but is not the only venue; other venues are admitted only after source, retention, schema, license, fidelity, and cost evidence passes.

The legacy `backfill-evidence-matrix.v1.toml` remains historical investigation evidence until the exhaustive coverage slice replaces it. Its Binance availability claims do not authorize Binance acquisition or replay.

Humans adjudicate novel or ambiguous usage/license terms. Machines fail closed unless the adjudicated record has the required evidence, allowed scope, passing status, and valid time window.

## Exclusive Data Ownership

- Historical acquisition owns backfill provider discovery, source selection, raw capture, and acquisition manifests.
- The NT catalog conversion lane owns projection of NT-native families into the NT catalog.
- Issues #24/#20 retain only their accepted normalized-lake layout and publish-contract scope; this decision does not add OI/liquidation schema, normalization, or collection work to them.
- Issue #158 retains its existing issue-defined REST/WS sidecar collection scope, including its named historical endpoints, subject to this decision's coverage policy. Its Binance cells remain `excluded_by_policy`. This decision adds no source-selection/coverage authority, canonical-schema ownership, or normalized-lake publisher ownership to it.
- A named #437 non-NT family-contract slice must define OI/liquidation units, availability semantics, canonical representation, and exactly one publisher/store owner before either family is implemented. Issue #158 must consume that representation within its existing policy-allowed scope; any additional handoff to #24/#20 or #158 requires an explicit tracker-scope update.
- Research Analytics consumes upstream artifacts read-only and owns only derived research outputs.

There is no separate Tier-C research-Parquet publisher and no second normalized OI store. Until the family-contract slice lands, canonical OI/liquidation publication is not authorized. Optional NT custom-data replay will read only the sole representation designated by that slice.

## Immutable Publication Protocol

A dataset publication contains immutable Parquet objects plus one canonical manifest. The final dataset identity is derived from sorted logical object descriptors without hashing a field that contains that identity.

Each manifest object binds:

- normalized relative path and derived final URI;
- byte length and SHA-256 content hash;
- S3 version ID and ETag;
- schema, source, window, instrument, and lineage metadata; and
- the config and implementation identity that produced it.

The sole writer uses conditional create. Its TOML object target is derived from measured encoder and replay peak RSS and stays below both worker-memory limits and S3's 5 GB single-`PutObject` ceiling. Multipart final writes are prohibited in v1. Future multipart support requires a client path and tests proving conditional `CompleteMultipartUpload`.

S3 `If-None-Match` is a collision guard, not the integrity authority. In a versioned bucket it can succeed after a delete marker becomes current. Exact manifest hash and version pins remain authoritative against delete-then-recreate.

The manifest is conditionally created last. Before that commit, the root is non-authoritative. A retry:

1. enumerates the uncommitted root;
2. adopts an expected existing object only after exact path, version, length, and SHA-256 verification;
3. creates missing expected objects;
4. rejects unexpected or mismatched objects; and
5. conditionally creates the manifest.

Kill-point tests cover every transition. TOML lifecycle rules govern abandoned uncommitted roots and incomplete multipart uploads. Committed roots are immutable.

## Manifest-Bound Read Protocol

A backtest run selects explicit dataset manifest URI and digest values. It never resolves a mutable latest pointer as input authority. The general Artifact Index may remain a discovery aid for other artifact families, but it cannot select or change the bytes used by a run.

The reader:

1. validates manifest schema, identity, requested coverage, normalized paths, uniqueness, lineage, and aggregate digest;
2. compares the committed root with the manifest and rejects pre-existing stray, missing, or mismatched objects;
3. selects only objects required by the run's venue, family, instrument, and window;
4. fetches exact S3 version IDs into an object-level content-addressed worker cache;
5. hashes bytes before atomically sealing a new cache entry; and
6. composes a read-only local NT catalog view from verified cache objects.

Cached objects are re-hashed exactly once per run at first use before reuse. A corrupt entry is deleted and refetched fail-closed. The per-run verification is amortized across all records that use the object.

The cache and local run view are non-authoritative and may be discarded. S3 objects are stored once; run manifests compose them by reference and never duplicate a full lake or dataset root.

The run-manifest-to-`BacktestNode` binding accepts only the sealed local view. Caller-provided raw S3 catalog paths and storage options are rejected. NT never independently relists a validated remote prefix. Tests cover stray files, missing files, current-version replacement, wrong version, length/hash mismatch, cache corruption, path traversal, and an NT pin-bump guard for the listing behavior that motivated this boundary.

## Required NT Capabilities

Before OI or comprehensive mixed-family replay is accepted, the pinned NT path must provide:

1. registered custom-data configuration and catalog query through `BacktestNode`, with missing registration failing closed; and
2. deterministic bounded-memory k-way merge across native and custom query iterators.

The merge order is `(ts_init, data-config ordinal, per-stream ordinal)` unless an NT-native stronger ordering contract is proven. Tests compare one-shot and streaming traces/results, equal-timestamp native/custom interleaving, and peak RSS bounded by configured streams/chunks rather than total rows. A pin guard asserts these exact capabilities on every NT revision change. A manual `BacktestEngine` loader is forbidden because it would create a second replay path.

## Funding And Open Interest Semantics

Funding stays NT-native through `FundingRateUpdate` and `on_funding_rate`. Its canonical record has a non-null effective `available_at`, records whether that time came from provider publication or capture, maps `available_at` explicitly to NT `ts_init`, and proves no-lookahead delivery.

The OI canonical schema designated by the non-NT family-contract slice must include:

- exact raw decimal value;
- required typed denomination/unit;
- contract multiplier and conversion provenance;
- snapshot or interval semantics;
- non-null effective `available_at`; and
- a provider-publication or capture fallback marker.

A derived common-notional value is a separate column and never replaces the raw venue value. If historical publication time cannot be established, the row remains research-only or pending proof rather than being backdated.

## AWS Cost And Operations

The TOML cost envelope is an admission and runtime limit, not a report-only estimate. It covers provider and requester-pays requests/bytes, regional transfer, S3 requests and storage, Inventory, hash reads, retry amplification, EC2, EBS, cache behavior, wall time, peak RSS, and cost per canonical GB. Crossing a configured limit stops the unit of work at a restart-safe boundary.

Workers run in the bucket region. Acquisition and conversion use bounded restartable chunks. Spot is initially limited to those proven restartable jobs. Backtests use On-Demand unless full retry fits the envelope; checkpoint/resume requires separate exact-state equivalence evidence.

Object-size and objects-per-canonical-GB gates prevent both memory-heavy files and another million-small-file catalog. Cold/warm cache startup, bytes read, hit rate, replay peak RSS, and wall time are pilot metrics.

Phase 0 uses an immediate one-time paginated all-version listing, then weekly all-version S3 Inventory for ongoing audit. Inventory is delayed reporting and is never runtime correctness evidence. Lifecycle policy must cover noncurrent versions, expired delete markers, abandoned/uncommitted objects, incomplete multipart uploads, and worker caches before fan-out. Destructive lifecycle activation and disposal of the stopped cross-region EC2/EBS/EIP resources require separate explicit owner approval.

No active legacy NT catalog is assumed. Any legacy root discovered by the all-version inventory is either fully hashed and manifest-adopted under a proven writer freeze, republished, or explicitly retired. There is no grandfather reader.

## Issue-Sized Implementation Order

1. **#437 authority slice:** adopt this decision, remove the superseded catalog plan from live authority, and reconcile only the historical-input clauses that conflict with explicit manifest binding.
2. **#563 conversion-state slice:** implement only its accepted per-source-object idempotency, delta, divergence, and recovery state. It does not own the immutable final-publication protocol.
3. **Named #437 immutable-publication slice:** integrate the existing #438 artifact-root and #439 manifest-mapping foundations with exact-set manifests, conditional-create final objects, crash recovery, and kill-point evidence. This explicit slice is the owner unless a narrower child issue is created before implementation.
4. **Named #437 manifest-read tracer:** bind one existing NT-native family through exact-version hydration, per-run cache verification, sealed local view, and `BacktestNode`.
5. **Named #437 NT replay-capability slice:** add typed custom-data catalog loading and bounded deterministic mixed-family merge; relate #836 without expanding its process-remediation scope.
6. **Named #437 coverage-authority slice:** implement the complete venue/product/table-family registry, Binance policy cells, interval validation, and funding/OI availability contracts.
7. **Named #437 non-NT family-contract slice:** define OI/liquidation canonical semantics and designate exactly one publisher/store owner; #158 then consumes that representation within its existing policy-allowed scope, while any additional #24/#20 or #158 handoff requires a tracker-scope update.
8. **Pilot slices:** run a semantic pilot across two evidence-approved non-Binance venues, followed by scale/restart/cold-warm-cache/cost proof before venue or family fan-out.

No PR combines these slices. Each PR names remaining accepted scope and its tracker.

## Supersession Boundary

This document supersedes `normalization-catalog-plan.v3.md` as the live historical acquisition and catalog-binding authority. That file is removed from the live tree and remains available in git history.

This decision does not globally delete the Artifact Index, producer ownership, or bulk-discovery behavior for results and derived artifacts. It only forbids mutable pointer resolution as backtest-input identity and forbids a second normalized-lake publisher. Issue #788 continues to own removal of default run-time source-proof ceremony; acquisition-time evidence does not reintroduce that ceremony into normal backtest execution.

Immutable proof artifacts and historical investigations remain provenance only. They do not override this decision.

## Acceptance

The architecture is implemented only when:

- one source and one coverage state exist for every governed cell/window;
- every Binance cell records the exact owner-policy exclusion and breadth loss;
- publication and every crash transition are create-only, recoverable, and manifest-last;
- every run is bound to exact manifest objects and re-verifies each reused cache object once;
- raw S3 catalog-path input and alternate replay paths are unreachable;
- native/custom mixed replay is deterministic and memory-bounded;
- funding and OI use first-knowable availability time without false unit equivalence;
- cost limits fail closed and both pilots pass; and
- the applicable behavior, negative, static, remote-CI, live-artifact, and exact-head evidence exists for each issue-owned slice.
