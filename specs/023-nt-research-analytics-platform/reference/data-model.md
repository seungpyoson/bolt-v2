# Data Model: Cross-Project Reference Package

This reference data model contains cross-project entities only. Project-owned
data models are defined inside the numbered project specs/plans:

- `1-backtesting-engine/`
- `2-research-analytics/`
- `3-dashboard/`

## EvidenceClaim

- `id`: Stable ledger id, e.g. `E-003`.
- `claim`: One assertion that may appear in specs, plans, tasks, or issue bodies.
- `status`: `SOURCE_PROVEN`, `USER_ASSUMPTION`, `GAP`, `DECISION_NEEDED`, or an
  explicit combined status when one row carries mixed facts.
- `evidence`: Exact file/doc/issue/source reference.
- `implication`: What a project may do because of the claim.
- `next_proof`: Concrete proof required before implementation or issue closure.

## NtCapabilityProof

- `surface`: Backtest, catalog, adapter, report, snapshot, data feed, execution,
  settlement, dashboard source, or analytics source.
- `venue`: TOML/registry-selected venue key; not a closed enum.
- `nt_pointer`: NT version/ref resolved by the target `bolt-v2` branch for
  proof.
- `bolt_pointer`: Target `bolt-v2` branch/ref used for proof.
- `status`: `SOURCE_PROVEN`, `USER_ASSUMPTION`, `GAP`, or `DECISION_NEEDED`.
- `api_refs`: NT Rust/Python/docs references.
- `compile_status`: not-run, passed, failed, or blocked.
- `behavior_status`: not-run, source-only, dry-run, testnet, no-submit, or live.
- `residual_gap`: Remaining unsupported surface, if any.

## ProviderMode

- `provider`: TOML/registry-selected provider key; not a closed enum.
- `venues`: Venue/data families covered.
- `data_classes`: Order book deltas, depth snapshots, trades, quotes, bars,
  instruments, fills, account state, settlements, on-chain fills, or metadata.
- `cost_components`: Subscription, AWS storage, compute, transfer, logs, query,
  dashboard, and reserve.
- `license_scope`: Personal, commercial, enterprise, public, unknown, or waived.
- `freshness`: Real-time, daily, monthly, archive, or forward-capture only.
- `fidelity_class`: Link to `DataFidelityClass`.
- `selected`: true only after cost and fidelity gates pass.

## DataFidelityClass

- `class`: `L2_REPLAY`, `TRADE_BAR_REPLAY`, `SIGNAL_ONLY`, or
  `FORWARD_CAPTURE_PENDING`.
- `allowed_claims`: What analysis/backtest result may claim.
- `forbidden_claims`: Claims blocked by missing data.
- `required_inputs`: Minimum source data required.
- `evidence_rows`: Ledger rows proving or blocking the class.

## RawEvidenceRecord

- `source_family`: Venue/provider/API/archive family.
- `source_uri`: Redacted or public URI.
- `capture_time`: UTC timestamp of capture.
- `source_time_range`: Data time range.
- `payload_hash`: Content hash or manifest hash.
- `schema_version`: Provider schema/version if known.
- `license_ref`: Provider/license evidence.
- `lineage_parent`: Parent record if derived from another source.

## CatalogProjection

- `projection_id`: Stable run id.
- `source_proof_id`: Accepted thin `SourceProofReport` that authorized this
  source/catalog input.
- `source_records`: Raw evidence records consumed.
- `nt_pointer`: NT version/ref resolved by the target `bolt-v2` branch for
  conversion.
- `catalog_path`: NT catalog path.
- `data_type`: NT data class.
- `instrument_ids`: NT instrument ids included.
- `transform_hash`: Transform/config hash.
- `fidelity_class`: Backtest-claim class.
- `validation_status`: not-run, passed, failed, or partial.

## SourceProofReport

- `source_proof_id`: Stable id for the proof gate.
- `source_proof_version`: Immutable version for this proof record.
- `status`: pending, accepted, or rejected.
- `supersedes_source_proof_id`: Prior proof id/version this record supersedes,
  if any.
- `latest_for_source_binding`: Whether this is the latest accepted proof for
  the source binding, or a pointer/index equivalent.
- `accepted_by`: Backtesting Engine/source-proof implementation authority that
  accepted the report, if accepted.
- `accepted_at`: UTC timestamp for acceptance, if accepted.
- `acceptance_mode`: automated or manual; automated acceptance is allowed from
  initial implementation only when every required check passes. Present only for
  accepted reports; pending or rejected reports omit the field.
- `required_checks`: Schema, sample, license, time/freshness, NT mapping,
  fidelity, and forbidden-claim check results.
- `fixture_type`: binary option or perps/spot.
- `source_binding_key`: TOML/registry-selected source key.
- `source_family`: Official API/archive, vendor archive, forward capture, or
  derived signal family.
- `coverage`: Instrument/market ids and source time range.
- `time_semantics`: Event time, availability time, capture time, and freshness.
- `schema_sample`: Schema version, field list, sample URI, and sample hash.
- `license_ref`: License/commercial-use proof and timestamp.
- `license_scope`: Personal, commercial, enterprise, public, unknown, or waived;
  accepted BTE catalog/backtest input requires public, commercial, enterprise,
  or waived scope.
- `nt_mapping_status`: accepted, rejected, signal-only, or pending.
- `fidelity_class`: L2_REPLAY, TRADE_BAR_REPLAY, SIGNAL_ONLY, or
  FORWARD_CAPTURE_PENDING.
- `forbidden_claims`: Claims this source must not support.
- `warnings`: Gaps, missing fields, limits, or unsupported coverage.

## ArtifactIndex

- `artifact_id`: Stable id for a canonical artifact.
- `artifact_kind`: Top-level kind: raw, nt_catalog, source_proofs, backtests,
  artifact_index, or research_analytics.
- `artifact_subfamily`: Optional subfamily inside the top-level kind. Required
  for Research Analytics artifacts: datasets, feature_tables,
  experiment_results, or promotion_packages.
- `producer_project`: Project or job family that produced the artifact and owns
  its index record.
- `manifest_uri`: Artifact-local structured manifest URI under `artifact_root`.
- `event_uri`: Immutable index event URI under
  `artifact-index/v1/events/kind=<artifact_kind>/`.
- `snapshot_id`: Immutable snapshot id when committed.
- `snapshot_uri`: Committed snapshot URI under
  `artifact-index/v1/snapshots/kind=<artifact_kind>/`.
- `latest_pointer_uri`: Generated latest-pointer URI under
  `artifact-index/v1/pointers/kind=<artifact_kind>/latest.json`.
- `content_hash`: `sha256` value; S3 ETag is not the content hash.
- `lineage_ids`: Source proof, catalog projection, run, dataset, or result ids
  plus parent artifact versions and `sha256` hashes for cross-kind traversal.
- `write_authority`: producer-owned, read-only-consumer, or unsupported.
- `commit_state`: staged, committed, orphan, or superseded.
- `lifecycle_state`: active or inactive; hot index pointer/current snapshot stays active.
- `audit_epoch_uri`: Optional pointer-swap audit record used for forensics, not
  normal discovery.

## ResearchAnalyticsArtifact

Research Analytics may write only these derived families under the
`artifact_root/research-analytics/v1/` prefix:

- `datasets`: point-in-time research datasets.
- `feature-tables`: point-in-time feature tables.
- `experiment-results`: experiment metadata, metrics pointers, consumed BTE
  result ids, leakage reports, verdict fields, and optional typed
  promotion-config refs for real GO findings.

Every RA-owned artifact records owner, schema version, source refs, source
hashes, content hash, lifecycle state, and Artifact Index event behavior. RA
does not write upstream raw, NT catalog, source-proof, or backtest records.

## IssueSlice

- `issue_title`: Proposed title.
- `project_directory`: One of `1-backtesting-engine`, `2-research-analytics`, or
  `3-dashboard`, unless the issue is explicitly cross-project evidence work.
- `existing_issue_relation`: updates, depends-on, blocks, duplicates, or new.
- `evidence_rows`: Ledger rows justifying the slice.
- `accepted_scope`: Exact work included.
- `residual_scope`: Explicitly out of scope or tracked elsewhere.
- `acceptance_evidence`: Files, commands, reports, or issue comments required.
- `review_packet`: Source packet for follow-up review if requested.
