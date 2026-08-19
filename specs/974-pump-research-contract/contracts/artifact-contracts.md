# Contract: Research Experiment Artifacts

## Index Placement

Add one `experiment-contracts` subfamily beneath the existing
`research-analytics` Artifact Index kind. All records use the existing configured
artifact root, immutable event objects, committed snapshots, latest-pointer
compare-and-swap, lifecycle fields, and SHA-256 lineage references.

The subfamily contains typed payloads; it does not create independent pointers
or storage roots for each payload type.

## Common Envelope

Every payload carries:

- `artifact_schema_version`
- `artifact_type`
- `artifact_id`
- `experiment_id`
- `experiment_version_id`
- `artifact_uri`
- `content_hash`
- `semantic_hash` when semantic bytes differ from retained bytes
- `byte_length`
- `created_at`
- `created_by_role`
- `lineage_refs`
- `source_entry_refs`
- `index_lifecycle_state` (existing storage `active`/`inactive` meaning)
- `evidence_state` (`active`, `quarantined`, `revoked`, `expired`, or
  `invalidated` as applicable)
- `invalidated_by_refs`

The envelope rejects a URI outside the configured artifact root, an incorrect
hash, missing lineage, unauthorized producer role, unknown type/version, or
lifecycle promotion from a terminal state.

## Artifact Types

- `experiment_definition`
- `roster_manifest`
- `source_register_snapshot`
- `commitment_g`
- `commitment_d`
- `commitment_c`
- `commitment_e`
- `commitment_p`
- `timestamp_receipt`
- `custody_event`
- `custody_checkpoint`
- `disclosure_program`
- `disclosure_receipt`
- `episode_manifest`
- `execution_attempt`
- `semantic_comparison`
- `research_report`
- `user_authorization`
- `atomic_claim_registry`
- `invalidation_event`

## Commit Protocol

1. Validate the typed payload and every referenced active ancestor.
2. Produce canonical semantic bytes and hashes.
3. Write the immutable payload using fail-on-dirty semantics.
4. Create the immutable Artifact Index event.
5. Build a deterministic snapshot containing the event.
6. Update the subfamily pointer only when its observed predecessor still
   matches. A stale predecessor fails; it is never overwritten.

Commit success does not itself satisfy a formal commitment. G/D/C/E/P also need
a verified independent timestamp receipt and, where specified, a closed custody
checkpoint.

## Custody Chain Contract

- Canonical evaluation bytes are readable only through the custodian-controlled
  access boundary. Runtime principals are authenticated outside payload data and
  matched to TOML-bound role/credential scopes. CLI fields cannot self-assign a
  role.
- Access uses generation-bound leases. Checkpoint closure stops new leases,
  drains every issued lease, records zero active leases, then captures the head.
  A later access attempt with an old generation is rejected and appended as a
  failed custody event.
- Event sequence starts from the G-declared genesis value.
- Each event hashes the exact previous event hash.
- Sequence gaps, duplicate sequences, unknown event kinds, invalid actor roles,
  missing input/output references, and a mismatched previous hash are terminal
  validation failures.
- A checkpoint fences relevant role access, captures the head, verifies a
  timestamp receipt for that head, conditionally verifies the head remains
  unchanged, and consumes a single-use authorization.
- A head change produces a `stale` checkpoint and requires a new attempt.
- Disclosure and successful-result bytes remain unavailable until their release
  checkpoint closes.
- Partial execution output is quarantined and indexed as such; it is never
  silently discarded or treated as a result.
- Canonical and verification execution use distinct authenticated principals and
  single-use capabilities. Test fixture principals and timestamp verifiers are
  unavailable to non-test builds.

## Lifecycle and Invalidation

Source register entries and dependent artifacts support `active`, `quarantined`,
`revoked`, and `expired`. Derived results and claims additionally support
`invalidated`. A lifecycle event traverses Artifact Index lineage and emits an
invalidation artifact listing every affected active descendant.

Evidence validity is independent of the existing Artifact Index storage
lifecycle. An inactive cold artifact may remain valid evidence; an active hot
artifact may be quarantined and unusable.

History is immutable. Re-admission, corrected evidence, or a new experiment
creates new versions and lineage; it never mutates an earlier artifact back to
active.

## Claim Compatibility

An artifact may narrow an upstream claim limit but cannot expand it. Publication
fails when a claim:

- labels an episode as manipulation without the required authority;
- uses `mechanism_consistent_with` without admitted E/P-selected granular
  evidence and survived falsifiers;
- uses `manipulation_proven` beyond a final adjudication/admission's exact scope;
- presents `not_proven` as `non_manipulated`;
- makes queue-position or L3 claims without admitted market-by-order data and
  venue semantics; or
- depends on a non-active source or invalidated ancestor.

Every report must enumerate its complete released population and denominators,
effect sizes and dependence-aware uncertainty, balance, missingness, attrition,
survivorship, controls, sensitivity cells, null/small-sample limits, prior
overlapping experiments and attempts, and temporal/instrument/venue
generalization scope. Mechanism reports also carry the E-authorship disclosure
and frozen claim-tier cap. Missing required disclosure blocks publication.

## Storage Contract

The canonical object store retains admitted discovery inputs or a lossless
normalized panel and later authorized bounded granular windows. Local work paths
are ephemeral and subject to the TOML storage budget. A run fails before ingest
when its declared retained and ephemeral byte bounds exceed the authorized
budget. No command supports a universal unbounded historical download.
