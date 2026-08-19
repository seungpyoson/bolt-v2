# Contract: Authoritative Experiment Definition

## Authority

Exactly one strict Bolt TOML document defines experiment meaning. NT run config,
provider metadata, notebooks, command-line flags, and environment variables may
not override semantic values. The CLI accepts a path to this document and
non-semantic output locations only.

Parsing rejects unknown keys and duplicate semantic identifiers. All runtime
values, including venues, time ranges, thresholds, windows, coverage minima,
roles, registry bindings, hash/canonicalization versions, retry limits, cost
caps, and storage bounds, come from typed TOML fields.

## Required Top-Level Tables

| Table | Purpose |
|---|---|
| `experiment` | Family/version ids, parent, schema, canonicalization, purpose, append authority |
| `target_frame` | Venue/market/time scope, roster construction, reconciliation and completeness |
| `roles` | Separated registered roles and non-secret credential-scope references |
| `storage` | Existing artifact root binding, lifecycle and bounded local-work limits |
| `timestamp_policy` | Registered verifier binding, accepted receipt schema, anchoring intervals |
| `source_policy` | Admission, retention, fidelity, correction and zero-spend rules |
| `partitions` | E0, discovery/evaluation narrowing, full-span purge and censoring |
| `detector` | Observation construction, clocks, trigger cells, warm-up, overlap and deduplication |
| `controls` | Risk sets, features, matching, balance, support, contamination and seeds |
| `analysis` | Estimands, hypothesis families, dependence, multiplicity and null-result rules |
| `disclosure` | Noninteractive program, budget, fields, schedule, suppression and accounting |
| `confirmation` | Primary-cell rule, comparator, normalization, tolerances and retry state machine |
| `enrichment` | Strata, draw, hypotheses, falsifiers, candidate packet, ranking and storage bounds |
| `claims` | Tier policies, required evidence, forbidden promotions and invalidation rules |

Repeated typed tables hold roster units, identity mappings, temporal assertions,
source-register versions, and—after the relevant gate—candidate datasets.

## Canonicalization

1. Parse strict TOML into typed Rust structures.
2. Validate all local and cross-table invariants.
3. Replace no missing values with implicit runtime defaults; schema-declared
   semantic constants are versioned in the schema implementation.
4. Serialize the semantic structure in declared field order. Dynamic mappings
   use ordered keys; set-like arrays are sorted by semantic id; ordered analysis
   arrays retain their declared order.
5. Encode UTF-8 without volatile paths, timestamps, comments, or formatting.
6. Hash the resulting bytes with the declared algorithm.

The artifact records the original TOML hash and canonical semantic hash. The
semantic hash identifies the experiment version.

## Cross-Table Validation

- Every referenced id resolves exactly once and belongs to the same experiment
  version or an explicitly pinned ancestor.
- Every roster unit has exactly one coverage status.
- Identity joins traverse time-valid mappings and never use ticker alone.
- All sources required by a canonical operation are active, unexpired, retained,
  and claim-compatible.
- E0 encloses the final evaluation range and G precedes all E0 access.
- Purge spans include every configured lookback and outcome window.
- D contains every detector, control, analysis, correction, and disclosure input
  required before discovery.
- C selects exactly one primary cell or D-frozen aggregation rule.
- E precedes candidate solicitation/access and includes the actual enrichment
  draw and mechanical selection rule.
- P references only candidates admitted under E and records the mechanical
  winner before mechanism-bearing access.
- Canonical transitions require verified timestamps and closed custody
  checkpoints.
- A user-authorization receipt is required for every Stage-2 quote, pilot, query,
  purchase, or nominally free provider access.

## Compile Boundary to NautilusTrader

The definition may compile an immutable NT run input containing admitted catalog
references, instruments, time ranges, model inputs, seeds, and output bindings.
The compiled input records its parent experiment version and hash. NT config may
not introduce another venue, source, time range, detector, control, estimand, or
claim rule. A mismatch fails before NT execution.

## Deferred Values

Concrete venues, dates, thresholds, cost caps, role identities, timestamp
authority, and provider candidates are intentionally absent from this design.
They must be user-approved typed values in an experiment version. Missing values
block the associated canonical operation; there is no fallback or guessed value.
