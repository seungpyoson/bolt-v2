# Current Decision-Evidence Rebuild Design

## Decision

Replace the existing decision-evidence runtime from current `main` with one current-only hard-cutover path. Do not port PR #1503's runtime modules, historical codecs, generic writer trait, JSON projection bridge, standalone preflight, or representative-only fixture strategy.

Pre-cutover evidence is never a runtime input. Operators archive it outside the configured data root after independently proving quiescence. The new binary neither migrates, deletes, repairs, nor decodes it.

## Scope

This is one atomic #1354 implementation slice:

- register every structurally reachable decision-evidence producer purpose;
- give every purpose one fresh current identity and one identity-owned codec;
- separate machine and observation sinks;
- replace all production writers and live/offline consumers;
- validate the active machine stream before constructing any append capability;
- preserve purpose-specific durability and action policies as typed outcomes;
- bound observation write-failure reporting;
- remove the old writer, readers, private Shadow-PnL parser, migration script, and old config key.
- migrate the separately resolved backtesting workspace from its private generic evidence writer to
  the same typed current codecs, recorder, durable sinks, and fact reader.

The slice does not implement the #1385 capacity, rotation, retirement, durable-ordinal, or restart-exact-once work. Live activation remains prohibited until independent order/fill/settlement quiescence can be proven and the repository's deployment gates are satisfied.

## Root Cause Being Removed

The rejected implementations described relationships in metadata while keeping bypassable generic Rust boundaries. Correctness depended on calling preflight before a public constructor, treating `Ok(())` as proof of durable append, not editing shared runtime wire types, and assuming representative fixtures froze an accepted byte domain.

The rebuild accepts a stronger criterion: invalid runtime paths must be unconstructible through the public API. Tests prove behavior at those boundaries; they do not compensate for an open boundary.

## Closed Runtime Boundary

`DecisionEvidenceRuntime::open(&LoadedBoltV3Config)` is the only production constructor. It performs, in order:

1. Validate the configured machine, observation, and retired paths as relative lexical components.
2. Open `catalog_directory` once and resolve every active and retired component through retained descriptor-relative `openat`/`fstatat` operations with no-follow semantics.
3. Reject any configured retired path that exists.
4. Open the machine and observation files relative to their retained parent descriptors and reject non-regular or identical underlying files.
5. Fully validate the bounded machine descriptor, rejecting oversized, torn, blank, old, unknown, observation, or malformed records, and construct typed recovery facts.
6. Fully validate the observation descriptor. Invalid retained observation content is preserved and constructs an explicitly poisoned observation sink; it never becomes readiness authority.
7. Seek healthy retained descriptors to their append positions.
8. Return a runtime containing the immutable startup recovery facts, typed observation status, and the only append-capable recorder.

There is no public writer constructor in the default live package and no separately callable startup preflight. The bytes validated for startup and the file later appended are the same open file description.

Diagnostic and test inspection uses `read_current_evidence_facts`. Every offline application
consumer uses its registered typed projection, including `read_shadow_pnl_events` and
`read_backtest_run_guard_events`. All routes share one exact-identity decoder and JSONL framing
check, and none can construct append capability. Shadow PnL and backtesting own no envelope parser,
kind dispatch, or generic fact reducer.

The backtesting vertical slice is a separate Cargo workspace and enables the
`offline-current-evidence` feature on its `bolt-v2` dependency. That feature alone exposes
`OfflineDecisionEvidenceRuntime::from_fresh_files`, which accepts only two empty, distinct file
descriptors and returns the same concrete recorder used by live strategy construction. The default
live build does not compile this constructor. Backtest run-guard projections read the resulting
registered `BacktestRunGuardEvent` values; they do not implement a writer trait, consume generic
facts, or maintain a second payload contract.

## Typed Write Boundary

The runtime exposes purpose-specific methods on one concrete `DecisionEvidenceRecorder`; it exposes no implementable writer trait.

Only the durable sink constructs `AppendReceipt`, and only after the record has been completely written and `sync_data` succeeds. Encoding rejection occurs before I/O and leaves the sink healthy. Any write or sync error returns phase-tagged `CommitIndeterminate`, permanently poisons that sink for the process lifetime, and makes later appends return `SinkPoisoned` without I/O. Callers never retry an indeterminate fact through the poisoned sink.

Purpose methods return policy-specific types:

- new-risk and reconciliation purposes return `Result<AppendReceipt, RecordFailure>`;
- risk-reducing and preserve-result purposes return `NonBlockingRecordOutcome::{Appended, Failed}`;
- observations return `ObservationRecordOutcome::{Appended, FailureReported, FailureSuppressed}`.

Callers must match these exhaustive types. New-risk actions cannot proceed without a receipt. Risk reduction continues while retaining an observable failure. No caller receives `Result<()>` when durability has more than one semantic outcome.

The recorder owns an `ObservationFailureEpisodes` set keyed by generated purpose. The first failure in a continuous per-purpose episode emits one error and returns `FailureReported`; later failures return `FailureSuppressed`. A successful append removes the purpose and permits one report for a later episode. This requires no time or rate hardcode.

Tests inject a durable append target below the recorder. They do not implement or counterfeit the recorder and cannot construct `AppendReceipt` directly.

## Immutable Current Identities

Every current identity has a dedicated private binding within its domain codec module containing:

- a private `LineV1` spelling every envelope and payload member;
- a private `PayloadV1` that owns its serialized field set;
- private frozen V1 wire enums and tagged variants used by that payload;
- exhaustive `TryFrom` conversions between semantic input/fact types and the frozen wire types;
- encode and decode implementations bound to the generated identity marker.

Top-level payload DTOs are never shared between identities, including shape-identical blocked/submit or exit-submit/exit-hold purposes. Frozen nested value types may be shared only when they have identical meaning and live in an explicitly versioned frozen-wire module. Frozen wire types never reference serde-derived runtime DTOs or runtime enums.

There is no `serde_json::Value` projection between runtime and wire types. Adding a runtime or frozen enum variant makes an exhaustive conversion fail compilation until the identity disposition is explicit.

The stable envelope uses exact equality on `(kind, schema_version)` and exact `gate_id`. `gate_version` is nonempty diagnostic metadata and never selects or rejects a codec. No version ordering, fallback, wildcard identity, `Other`, or legacy dispatch exists.

## Registry Authority

TOML owns finite current membership and relationships:

- every structural producer ID to exactly one purpose; a purpose may have multiple registered producers;
- purpose owner, duties, sink, effect policy, novelty capability, current identity, and fact;
- exact identity metadata;
- consumer metadata;
- every fact-by-consumer disposition, including explicit irrelevant decisions.

Generated Rust owns sealed markers, exhaustive enums and matches, and completeness/uniqueness validation. Adding a producer, purpose, identity, fact, or consumer invalidates generation until every required edge is supplied.

TOML contains no Rust function paths or executable projections. Handwritten identity modules bind generated markers through Rust trait implementations; missing or duplicate bindings fail compilation.

## Verification Contract

Each implementation checkpoint starts with a failing behavior or compilation test and ends green before the next checkpoint.

Required evidence includes:

- atomic runtime opening rejects retired, symlinked, non-regular, aliasing, old, unknown, observation-in-machine, malformed, torn, blank, exact-cap-plus-one, and changed-file cases;
- startup recovery facts and later appends use the retained validated machine descriptor;
- injected partial-write and sync failures never produce `AppendReceipt`, poison only their sink, and make later attempts byte-preserving refusals;
- invalid retained observation content remains unchanged and poisons only the observation sink while machine recovery remains available;
- every effect-policy outcome reaches its caller without erasure;
- observation failures report once per continuous purpose episode and reset after success;
- every producer emits exactly one registered identity to its registered sink;
- each identity has byte-exact canonical fixtures for every enum branch and optional null/present state; admitted omitted forms are separately frozen and canonicalize to explicit-null bytes;
- each required field has missing and wrong-type rejects; unknown fields, unknown enums, wrong gate, wrong exact pair, boundary violations, and contradictory semantic combinations reject;
- old-only and mixed old/current active streams fail closed;
- observation floods do not change machine-stream bytes or recovery results;
- submit reservation and settlement/booking/terminal recovery reconstruct after restart;
- Shadow PnL consumes the shared typed reader and handles blocked observations by registered irrelevance;
- the backtest run guard consumes its registered typed reader, with an explicit disposition and typed reducer for every current fact;
- generator output is byte deterministic;
- the full unfiltered exact-head suite, formatting, clippy, and build checks complete successfully.

No new test inspects Rust source text. Privacy, sealed types, exhaustive matches, behavior tests, and raw-byte fixtures provide the evidence.

## Implementation and Review Sequence

Implementation remains one unmerged branch and one replacement PR so no partial runtime or dual authority reaches `main`. Internally it proceeds through independently reviewable commits:

1. Closed registry and generated markers.
2. Atomic runtime plus typed recorder and policy outcomes.
3. Identity-owned current codecs and complete fixture corpus.
4. Producer and consumer cutover, followed by deletion of the old path.
5. Full verification, internal adversarial review, push, and required native review.

An adversarial review occurs after each checkpoint while the diff is still small. A completed-plan checkbox requires its named evidence, not merely the presence of code.

## Cutover and Accepted Losses

The active config uses fresh machine and observation paths and lists the old path as retired. Presence of the retired path or any non-current active byte refuses startup. Ordinary startup never archives or deletes files.

Operators retain pre-cutover evidence as checksummed, read-only, runtime-inert archives. The new runtime cannot recover or query it. Entry-chain and Shadow-PnL continuity across the boundary is intentionally lost. Rollback after the first current machine record is pause and forward-fix only.

These accepted losses do not authorize losing active orders, fills, reservations, positions, booking transitions, settlements, accounting records, or required records. Live cutover requires independent external proof that none remain in flight.
