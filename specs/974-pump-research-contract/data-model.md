# Data Model: Manipulated Pump Research Contract

The human-authored source is one strict TOML `ExperimentDefinition`. Every
canonical entity below is serialized to a versioned deterministic semantic form,
content-addressed with SHA-256, and registered under the existing artifact root.
Identifiers are semantic identifiers, never storage paths or ticker-only keys.

## ExperimentVersion

Represents one immutable semantic version of the research definition.

- `experiment_id`: Stable experiment-family identifier.
- `version_id`: Hash-derived identifier for this semantic version.
- `parent_version_id`: Immediately preceding version; absent only at genesis.
- `schema_version`: Canonical definition schema.
- `canonicalization_version`: Deterministic serialization rules.
- `hash_algorithm`: Declared content algorithm; initially configured as SHA-256.
- `definition_hash`: Hash of canonical semantic bytes.
- `created_at`: Recorded creation time, not proof of independent ordering.
- `append_role`: Registered role allowed to propose this version.
- `purpose`: Exploratory, confirmatory, reproduction, audit, or mechanism study.
- `state`: Current derived experiment state.
- `lineage_refs`: Parent definition and admitted evidence references.

Validation:

- Unknown TOML fields, missing versions, duplicate semantic ids, unordered map
  ambiguity, broken parent links, and unauthorized append roles fail closed.
- The effective role is derived from an authenticated runtime principal and must
  match the TOML role binding; role names in payloads are assertions to verify,
  not authentication.
- A new version never edits or replaces its parent.
- Any post-C change creates a new version and cannot reuse exposed evaluation
  information except through a precommitted sequential family.

State transitions:

```text
draft
  -> genesis_committed
  -> discovery_committed
  -> discovery_released
  -> confirmation_committed
  -> confirmation_released
  -> enrichment_committed
  -> provider_selection_committed
  -> mechanism_released

any pre-release state -> exploratory
any active state      -> invalidated
```

Only verified commitment/checkpoint artifacts permit forward transitions.
`exploratory` and `invalidated` preserve lineage and cannot transition back.

## TargetFrame

Defines the population to which discovery results may refer.

- `frame_id`: Semantic identifier.
- `venue_keys`: TOML registry-selected venues.
- `market_family_keys`: TOML registry-selected market families.
- `start_time` / `end_time`: Closed-open event-time bounds.
- `time_unit_grain`: Unit at which roster coverage is classified.
- `outer_roster_rule`: Deterministic construction rule.
- `roster_vintage`: Input cutoff for the inventory.
- `inventory_source_refs`: Candidate/admitted inventory artifacts.
- `reconciliation_rule`: Conflict and precedence algorithm.
- `completeness`: Proven complete, enumerated incomplete, or unknown.
- `disclosure_text`: Required bounded-generalization statement.

## RosterUnit

One venue-instrument-time unit in enumerated roster R.

- `roster_unit_id`: Stable hash of frame, venue instrument, and time unit.
- `frame_id`: Parent frame.
- `venue_instrument_id`: Venue-native instrument identity.
- `time_unit`: Exact event-time interval.
- `status`: Exactly one of `eligible_observed`, `known_ineligible`,
  `known_insufficient_coverage`, or `existence_or_coverage_unknown`.
- `status_reason`: Typed reason plus evidence references.
- `coverage_metrics`: Expected, observed, missing, duplicated, and interrupted
  counts/intervals.
- `assertion_refs`: Append-only facts used to assign status.

Every enumerated unit must appear exactly once in a released roster manifest.

## IdentityNode and IdentityMapping

Prevents symbol reuse, migration, and rebrand histories from being conflated.

`IdentityNode` fields:

- `identity_id`: Stable identity.
- `identity_kind`: Venue instrument, token contract, or economic asset.
- `namespace`: Venue, chain, or economic-asset registry.
- `native_key`: Native instrument id, chain/address pair, or asset id.

`IdentityMapping` fields:

- `mapping_id`: Semantic mapping identifier.
- `from_identity_id` / `to_identity_id`: Distinct identity nodes.
- `valid_from` / `valid_until`: Event-time validity.
- `availability_time`: Earliest evidenced availability to the experiment.
- `retrieval_time`: When Bolt received the assertion.
- `status`: Active, superseded, disputed, or retracted.
- `confidence`: Declared evidence confidence, not a causal probability.
- `evidence_refs`: Archival or retrieval-time-attested evidence.
- `splice_rule`: Explicit allow/deny and transformation for continuous series.

Ticker-only joins are invalid.

## TemporalAssertion

Append-only fact used for universe, labels, mappings, or authoritative outcomes.

- `assertion_id`: Content-derived identifier.
- `subject_id` / `predicate` / `value`: Typed fact.
- `valid_time`: Event-time applicability.
- `publication_time`: Original or first-known publication time when evidenced.
- `availability_time`: Earliest evidenced availability to the experiment.
- `retrieval_time`: Capture time.
- `availability_status`: `archivally_attested` or `retrieval_time_attested`.
- `revision_of`: Earlier assertion when correcting or retracting.
- `assertion_state`: Active, corrected, retracted, or disputed.
- `evidence_refs`: Retained proof.

`retrieval_time_attested` facts cannot support contemporaneous-availability or
predictive claims.

## SourceRegisterEntry

The dataset-specific admission decision. Provider brand is insufficient.

- `source_entry_id` / `source_entry_version`: Immutable identity and version.
- `dataset_product_version`: Exact dataset and version.
- `upstream_provenance`: Original source and transformations.
- `query_and_fields`: Exact acquisition/query contract.
- `coverage_contract`: Venues, instruments, dates, fields, and gap semantics.
- `rights_packet`: Query, download, cache, post-termination retention, derived
  data, collaboration, publication, attribution, and upstream rights.
- `fidelity_packet`: Timestamp, sequence, snapshot/reset, disconnect, raw
  payload, completeness, correction, and NT mapping evidence.
- `cost_status`: Zero-cost verified, quoted, paid-authorized, or unknown.
- `retained_artifact_refs`: Exact raw or lossless normalized inputs.
- `reviewer` / `decision_time` / `expiry_time`: Admission governance.
- `state`: Active, quarantined, revoked, or expired.
- `allowed_claims` / `forbidden_claims`: Non-expandable evidence limits.

Transitions:

```text
active -> quarantined | revoked | expired
quarantined -> revoked | expired
```

Re-admission creates a new version and a complete new evidence packet; it does
not reactivate the old entry.

## EvidenceArtifact

Immutable retained input or output.

- `artifact_id`: Stable semantic id.
- `artifact_type`: Typed experiment-contract artifact.
- `schema_version`: Payload schema.
- `artifact_uri`: URI beneath the configured artifact root.
- `content_hash`: SHA-256 over retained bytes.
- `semantic_hash`: SHA-256 over canonical semantic content when different.
- `byte_length`: Exact retained size.
- `lineage_refs`: Content hashes of every direct input.
- `source_entry_refs`: Admission records authorizing source use.
- `index_lifecycle_state`: Existing Artifact Index storage state, active or
  inactive.
- `evidence_state`: Active, quarantined, revoked, expired, or invalidated.
- `created_by_role`: Registered producer role.
- `created_at`: Event record time.

## Commitment

Common envelope for G, D, C, E, and P.

- `commitment_id`: Hash-derived identifier.
- `commitment_kind`: G, D, C, E, or P.
- `experiment_version_id`: Exact definition version.
- `payload_hash`: Canonical commitment hash.
- `custody_checkpoint_id`: Transactionally closed head when required.
- `timestamp_receipt_id`: Verified independent receipt.
- `authorized_by_role`: Registered governance role.
- `predecessor_commitment_refs`: Required prior commitments.
- `status`: Draft, timestamp_pending, verified, invalid, superseded, or consumed.

Kind-specific payloads:

- **G**: E0, role separation, disclosure program/budget, access schema,
  timestamp/anchor intervals, contamination and narrowing rules, prior-exposure
  inventory.
- **D**: Final frame/roster/source vintages, partitions, detector grid, primary
  estimand, controls, multiplicity, enrichment strata, correction and null rules.
- **C**: One primary cell/aggregation, final program, conformance checks,
  comparator, normalization/tolerance, retry state machine, minimal visible
  failure-code vocabulary/disclosure accounting, closed custody head.
- **E**: Mechanism predictions/falsifiers, sample, candidate packet, acceptance
  metrics, ranking/ties, fields, estimands, cost and rights requirements.
- **P**: Mechanically selected admitted dataset, fields, windows, fusion,
  disagreement/precedence/exclusion, and prior-exposure classification.

## TimestampReceipt

- `receipt_id`: Content-derived identifier.
- `verifier_registry_key`: TOML-selected registered verifier.
- `subject_hash`: Commitment or custody-head hash.
- `issued_at`: Authority time.
- `receipt_bytes_hash`: Retained receipt hash.
- `verification_record_hash`: Archived verifier result.
- `verification_time`: Local verification time.
- `status`: Pending, verified, invalid, expired, or unknown_authority.

Only `verified` receipts can advance a state transition.

## CustodyEvent and CustodyCheckpoint

`CustodyEvent` fields:

- `event_id`: Hash of canonical event content.
- `sequence`: Monotonic ledger sequence.
- `previous_event_hash`: Exact predecessor.
- `event_kind`: Ingest, access, disclosure, execute, retry, compare, quarantine,
  credential event, authorize, consume authorization, or unseal.
- `actor_role`: Registered role; actor identity is recorded separately.
- `credential_scope_id`: Non-secret credential-set reference.
- `input_refs` / `output_refs`: Content-addressed artifacts.
- `event_time`: Recorded operation time.
- `status` / `reason_code`: Typed outcome.

`CustodyCheckpoint` fields:

- `checkpoint_id`: Hash-derived identifier.
- `purpose`: Pre-D, pre-C, disclosure, pre-E, pre-P, or release.
- `locked_head_hash`: Head observed while role access is fenced.
- `timestamp_receipt_id`: Verified independent head receipt.
- `compare_and_swap_result`: Unchanged or changed.
- `single_use_authorization_id`: Authorization consumed at success.
- `fence_generation`: Custodian lease generation that stopped and drained access.
- `active_lease_count`: Must be zero at head capture.
- `closed_sequence`: Final included event sequence.
- `status`: Pending, closed, stale, invalid, or consumed.

A changed head, nonzero active lease count, mismatched authenticated principal,
or reused authorization makes the checkpoint invalid/stale and requires a new
checkpoint.

## DisclosureProgram and DisclosureReceipt

`DisclosureProgram` freezes exact output tables, groupings, filters, schedule,
release count, suppression, rounding, censoring, boundary flags, cross-query and
cross-version accounting, and deterministic program hash.

`DisclosureReceipt` records the program/version, input vintage, parameters,
output hash, cumulative budget, recipients, sequence, checkpoint, and delivery
status. Delivery is impossible until its checkpoint is closed.

## Episode and EpisodeManifest

`Episode` fields:

- `episode_id`: Stable semantic identity across repeated executions.
- `frame_id` / `roster_unit_id` / `venue_instrument_id`: Scope.
- `event_anchor`: Frozen anchor clock.
- `feature_cutoff`: Last observation eligible for trigger features.
- `trigger_completion_time`: Time all trigger conditions become known.
- `label_availability_time`: Giveback-window completion.
- `decision_time`: Later use in an experiment or report.
- `observation_window_refs`: Pump, giveback, baseline, warm-up, and purge spans.
- `trigger_cell_id`: Frozen detector-cell identity.
- `coverage_status`: Complete, left_censored, right_censored, or insufficient.
- `deduplication_group`: Frozen overlap/cooldown identity.
- `metrics`: Deterministic return, reported-volume, giveback, baseline, and
  missingness values.
- `claim_status`: `episode_detected`; never automatically manipulation.

`EpisodeManifest` contains every detected, censored, excluded, unmatched, and
insufficient candidate; roster and source denominators; controls; attrition;
hashes; and lineage. A null manifest is valid and publishable.

## ControlMatch

- `treated_episode_id`: Episode being matched.
- `control_roster_unit_id`: Same-time eligible risk-set unit.
- `pseudo_anchor`: Anchor assigned without future outcomes.
- `feature_cutoff`: Latest permitted matching information.
- `feature_vector_hash`: Canonical pre-anchor features.
- `distance` / `caliper` / `reuse_count`: D-frozen matching results.
- `balance_status`: Passed, failed, or not_evaluable.
- `common_support_status`: Supported or unsupported.
- `contamination_status`: Clean, contaminated, or unknown.
- `match_status`: Matched, unmatched, or excluded.

Future outcomes cannot affect eligibility or matching.

## ExecutionAttempt and SemanticComparison

`ExecutionAttempt` records canonical/verification role, environment and
dependency identities, exact inputs, seed/numeric rules, attempt number, output
refs, exposure status, and terminal/retryable/quarantined outcome.

`SemanticComparison` records both attempt refs, comparator version, normalized
semantic hashes, tolerance decisions, mismatch details, and equal/not-equal
status. It never chooses a preferred output.

Successful release requires both attempts complete, semantic equality, no human
result exposure, and a closed transactional release checkpoint.

## EnrichmentDraw and UserAuthorization

`EnrichmentDraw` freezes census/sample choice, deterministic strata, cases,
matched controls, near-threshold and negative controls, inclusion probabilities,
randomization, substitutions, unavailable coverage, weighting, and estimand.

`UserAuthorization` is a distinct immutable receipt containing authorized stage,
candidate scope, maximum cost, minimum usable coverage, permitted operations,
storage/retention bounds, expiry, and the exact Stage-1 evidence packet reviewed.
It contains no credential material. No quote, pilot, query, or purchase is valid
without a matching active receipt.

## ResearchReport

The immutable report artifact binds the released result to its full evidentiary
context.

- `report_id` / `report_version`: Immutable identity and version.
- `experiment_version_id`: Exact experiment meaning.
- `commitment_refs`: G/D/C and, for mechanism reports, E/P.
- `episode_manifest_ref`: Full detected/censored/excluded/control population.
- `execution_and_comparison_refs`: Every canonical/verification attempt and the
  released semantic comparison.
- `atomic_claim_refs`: Claims made by the report.
- `estimands`: Frozen definitions, point estimates, effect sizes, and units.
- `uncertainty`: Method, intervals, dependence assumptions, clusters, overlapping
  windows, common-market shocks, and small-sample limitations.
- `diagnostics`: Balance, missingness, attrition, survivorship, common support,
  sensitivity cells, controls, null results, and competing explanations.
- `generalization_scope`: Temporal, unseen-instrument, and unseen-venue limits.
- `prior_experiment_refs`: Every overlapping version, attempt, exposure, and
  multiplicity/claim-tier consequence.
- `label_policy`: Positive-unlabeled treatment of authoritative cases and an
  explicit ban on population-recall inference from unlabeled episodes.
- `mechanism_disclosure`: For Stage 2, records that E was authored after Stage-1
  identities/results but before granular content and applies the frozen tier cap
  to predictions derivable from Stage-1-visible fields.
- `source_and_lineage_refs`: Active evidence and retained artifact hashes.
- `limitations`: Required evidence-quality and causal limitations.
- `state`: Active, invalidated, superseded, or retracted.

Null, small-sample, mismatch, leakage-failed, and insufficient-evidence reports
remain publishable with the corresponding state and cannot be relabeled as
confirmatory success.

## AtomicClaim

- `claim_id` / `claim_version`: Immutable claim identity and version.
- `statement`: One bounded assertion.
- `tier`: Episode detected, not proven, manipulation alleged, venue sanctioned,
  mechanism consistent with, or manipulation proven.
- `scope`: Instruments, venues, actors, periods, experiment and evidence versions.
- `predicted_observations` / `minimum_evidence`: Predeclared support contract.
- `supporting_evidence_refs`: Active admitted evidence.
- `identity_certainty` / `timing_certainty`: Explicit evidence limits.
- `competing_explanations` / `falsifiers`: Required alternatives.
- `author_role` / `approval_role`: Separated where required.
- `state`: Active, invalidated, superseded, or retracted.

`manipulation_proven` requires a scoped final adjudication or explicit admission.
Source quarantine, revocation, or expiry traverses lineage and invalidates every
dependent active claim without deleting earlier versions.

## Relationships

```text
ExperimentVersion 1---1 TargetFrame
ExperimentVersion 1---* Commitment
TargetFrame       1---* RosterUnit
RosterUnit        *---* IdentityNode through IdentityMapping
SourceRegisterEntry 1---* EvidenceArtifact
Commitment        *---1 TimestampReceipt
Commitment        *---1 CustodyCheckpoint
CustodyCheckpoint 1---* CustodyEvent (through closed sequence/head)
ExperimentVersion 1---* EpisodeManifest
EpisodeManifest  1---* Episode
Episode          1---* ControlMatch
Commitment C      1---2 ExecutionAttempt
ExecutionAttempt 2---1 SemanticComparison
Commitment E      1---1 EnrichmentDraw
UserAuthorization 1---* authorized Stage-2 operations
AtomicClaim       *---* EvidenceArtifact
ResearchReport    1---* AtomicClaim
ResearchReport    1---* ExecutionAttempt
```
