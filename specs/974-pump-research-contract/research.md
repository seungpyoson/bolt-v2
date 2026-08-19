# Phase 0 Research: Manipulated Pump Research Contract

This document resolves the architectural choices required to plan the feature.
It does not select a data provider, authorize spend, choose experiment values,
or claim that any observed episode was manipulation.

## Decision 1: Extend the Existing NT Research Path

**Decision**: Implement the contract inside
`crates/backtesting-vertical-slice`, reusing `source_proof`, `artifact_index`,
`artifact_store`, `research_analytics`, `run_manifest`, and `result_contract`.

**Rationale**: These modules already represent accepted sources,
content-addressed lineage, one artifact root, research verdicts, and NT-native
run inputs. The repository governance assigns replay, events, fills, snapshots,
and reports to NautilusTrader.

**Alternatives considered**:

- QuantConnect or another cloud backtester: rejected as a second canonical
  simulation path and insufficient for the required historical granular claims.
- A new Python/notebook execution service: rejected by the pure-Rust and
  read-only-notebook boundaries.
- A vendor UI as truth: rejected because experiment meaning and retained lineage
  would no longer be controlled by Bolt.

## Decision 2: One Strict TOML Definition, One Canonical Semantic Form

**Decision**: The human-authored authority is one strict, versioned Bolt TOML
document. Rust parses it with unknown-field rejection into typed structures,
validates cross-field invariants, and serializes a semantic payload with ordered
fields and ordered maps to deterministic bytes. SHA-256 identifies those bytes.

**Rationale**: TOML satisfies repository configuration policy, while hashing a
typed semantic form prevents whitespace, comments, or table ordering from
creating distinct experiments. The exact canonicalization version and hash
algorithm are recorded with every artifact.

**Alternatives considered**:

- Hash raw TOML: rejected because semantically identical formatting changes
  would create different identities.
- Accept TOML and JSON inputs: rejected as dual configuration paths.
- Leave map order implementation-defined: rejected because reproducibility must
  cross environments.

## Decision 3: Add One Artifact Index Subfamily

**Decision**: Add `experiment-contracts` beneath the existing
`research-analytics` Artifact Index kind. A typed `artifact_type` distinguishes
experiment versions, commitments, custody events/checkpoints, roster/source
manifests, episode manifests, authorization receipts, and claim-registry
versions. All share one snapshot and latest-pointer protocol.

The existing Artifact Index `active`/`inactive` field continues to mean storage
lifecycle. Research evidence validity is a separate typed field with
`active`/`quarantined`/`revoked`/`expired`/`invalidated` states, so rights or
integrity changes cannot be mistaken for hot/cold storage transitions.

**Rationale**: Commitments must be registered before results exist. Reusing the
existing index commit and conditional-pointer mechanics preserves one artifact
root and one discovery path while avoiding several new top-level kinds.

**Alternatives considered**:

- Store everything as `experiment-results`: rejected because a pre-result
  commitment cannot depend on a future result artifact.
- Add a top-level artifact kind for every entity: rejected as unnecessary
  verifier and index expansion.
- Use an unindexed local ledger: rejected because it cannot provide durable
  lineage or cross-run discovery.

## Decision 4: Acquisition Is Outside Bolt; Admission Is Inside Bolt

**Decision**: Bolt accepts only staged candidate artifacts and metadata. It does
not call Surf, CoinAPI, Tardis, Dune, Allium, Arkham, exchanges, or another data
provider in this feature. Token-screener may produce candidates, but Bolt makes
the admission, retention, fidelity, coverage, cost-status, and lifecycle
decision before canonical use.

**Rationale**: This preserves zero incremental spend, avoids a provider choice,
and prevents acquisition convenience from becoming research truth.

**Alternatives considered**:

- Add a Surf adapter now: rejected because Surf Data API is discovery evidence,
  Surf Chat is unsuitable for deterministic automation, and rights/recall remain
  unresolved.
- Buy a full historical L2/L3 archive: rejected before Stage 1 quantifies useful
  windows and a candidate passes exact rights/fidelity gates.
- Treat official/free data as automatically canonical: rejected because free
  sources still have coverage, revision, retention, and fidelity risks.

## Decision 5: Bound Storage by Evidence Windows

**Decision**: Store immutable coarse discovery inputs or a lossless normalized
panel in the configured object store. Any later granular data is limited to the
E-frozen event, matched-control, near-threshold, negative-control, and warm-up
windows. Conversion and replay workers use ephemeral local storage; retained raw
and derived objects are content-addressed.

**Rationale**: A complete two-to-three-year L2/L3 archive is unnecessary for the
first research question and creates large cost and disk obligations. Exact
Stage-1 windows provide a measurable basis for later authorization.

**Alternatives considered**:

- Universal local archive: rejected for storage and operational burden.
- Retain only derived results: rejected because detection and replay would not
  be reproducible.
- Cache provider responses without explicit rights: rejected by source admission.

## Decision 6: Use a Registered Timestamp Verifier and Fail Closed

**Decision**: Formal commitments carry canonical bytes, SHA-256, a typed
timestamp receipt, and a verifier registry key selected through TOML. The
implementation verifies an externally produced receipt through the registered
verifier boundary. Missing, pending, expired, unknown, or invalid receipts block
the transition. A hosted repository event is corroboration only.

**Rationale**: The contract needs independent ordering evidence, but selecting a
timestamp vendor or service is outside the approved scope. A fail-closed verifier
interface lets the schema and state machine proceed without a hidden default.

**Alternatives considered**:

- Git commit or PR time only: rejected because the spec forbids a hosted
  repository event as sole authority.
- Hardcode one authority: rejected because concrete providers are deferred
  user-approved registry values.
- Allow unverified commitments temporarily: rejected because later verification
  cannot prove the gate was not crossed early.

## Decision 7: Custody Uses Append-Only Events and Conditional Checkpoints

**Decision**: Every access, disclosure, execution, retry, comparison, credential
event, and unsealing action appends a hash-linked event. A checkpoint locks or
fences the relevant role, captures the current head, obtains a verified
independent timestamp, conditionally confirms that the head is unchanged, and
consumes one authorization exactly once. Changed heads restart; omitted or stale
tails invalidate the affected scope under the G-frozen propagation rule.

Canonical evaluation bytes remain behind a custodian-controlled access boundary.
An operation derives its authenticated principal from the execution environment
and matches it to the TOML-bound role/credential scope; a caller cannot assert a
role through a command flag or artifact field. The custodian stops new leases,
drains issued leases, closes the ledger head, and issues a scoped single-use
capability only after the checkpoint succeeds. Canonical and verification roles
receive distinct capabilities. Fixture authorities and principals are compiled
for tests only and cannot satisfy a non-test transition.

For non-test execution, the principal source is AWS STS `GetCallerIdentity`
through the already locked SDK. G binds accepted AWS principal/role identities
without storing credentials. Unavailable STS identity, a mismatched binding, or
an assumed-role/session shape outside the committed rule fails closed. AWS
runtime identity is infrastructure authentication for the existing SSM/object
store boundary, not an alternate product-secret source.

**Rationale**: This directly closes the reviewer's unanchored-tail concern and
can reuse existing content-addressed writes and conditional pointer updates.

**Alternatives considered**:

- Periodic mutable audit log: rejected because access can occur between the last
  log write and release.
- Timestamp only the commitment file: rejected because it does not close the
  associated access/disclosure tail.
- Human sign-off without authenticated mechanical fencing: rejected because it
  is not deterministic, enforceable, or replayable.

## Decision 8: Deterministic Retrospective Detection Is Not Manipulation Proof

**Decision**: Stage 1 implements a TOML-defined retrospective detector requiring
abnormal return, abnormal reported volume, and subsequent giveback. It records
all clocks, censoring, overlap, deduplication, coverage, and same-time matched
controls. Its strongest automatic statement is `episode_detected`; the absence
of authority evidence is separately `not_proven`.

**Rationale**: A reproducible anomaly definition is useful even when actor intent
and mechanism are unknown. Separating observation from legal/causal proof avoids
the shallow causal claims that motivated the project.

**Alternatives considered**:

- AI classification of manipulation: rejected as nondeterministic and
  unsupported by source-level evidence.
- Price-only trigger: rejected because it omits the frozen volume and reversal
  contract.
- Tune thresholds after evaluation: rejected as hindsight leakage.

## Decision 9: Confirmation Is One Sealed Program, Reproduced Twice

**Decision**: C identifies one primary cell or D-frozen aggregation rule. One
canonical execution and one credential/environment-independent verification
execution consume identical committed inputs. The comparator operates on
semantic outputs after committed normalization. A mismatch, human result
exposure, exceeded retry cap, or unanchored access tail is terminal for
confirmatory release.

**Rationale**: Two independently operated executions test reproducibility while
avoiding false claims that two runs are independent analytical methods.

**Alternatives considered**:

- Choose the better result: rejected as outcome selection.
- Retry until matching: rejected because it creates an adaptive path.
- Release intermediate metrics: rejected unless they were part of G's fixed
  disclosure budget and checkpointed first.

## Decision 10: Stage 2 Selects Mechanism Evidence Before Seeing It

**Decision**: E freezes hypotheses, falsifiers, sampling, candidate packet,
acceptance metrics, ranking, ties, and cost/rights fields before quote, pilot, or
granular access. Separate explicit user authorization follows Stage-1 counts and
cost/storage bounds. P then records the mechanically selected admitted dataset
before mechanism-bearing access.

**Rationale**: Provider coverage and selective availability can otherwise choose
the mechanism story after cases are known. The E/P split prevents a technically
successful pilot from silently becoming the preferred evidence source.

**Alternatives considered**:

- Pick the cheapest provider first: rejected because low price cannot repair
  missing sequence, raw payload, rights, or coverage.
- Enrich detected cases only: allowed only for named case studies, not population
  or discrimination claims.
- Treat L2 as L3: rejected; queue-position claims require admitted
  market-by-order evidence and venue semantics.

## Decision 11: Implement as Spec-Bound Vertical Slices

**Decision**: Deliver seven ordered slices: definition/index, roster/admission,
prospective G/D custody, Stage-1 detector, C/sealed confirmation plus the primary
report, E/P authorization, and claim/mechanism-report lifecycle. Each slice has
behavior and fail-closed tests and names its remaining accepted scope.

Each branch and PR names this spec and one slice. A separate GitHub issue is
optional; the named slice itself satisfies repository scope governance and avoids
coordination artifacts that do not reduce research or trading risk.

**Rationale**: The full contract is too broad for one reviewable PR. The ordering
creates useful evidence at each step without pretending later authority exists.

**Alternatives considered**:

- One broad implementation PR: rejected by repository scope discipline.
- Start with provider integration: rejected because it skips the experiment and
  admission contracts that make provider data useful.
- Start with a strategy: rejected because this feature is retrospective research,
  not predictive or live-trading behavior.

## Resolved Unknowns

There are no remaining architecture-level unknowns. Concrete
experiment values and external services are deliberate gated inputs, not design
unknowns. Their absence blocks the corresponding canonical transition.
