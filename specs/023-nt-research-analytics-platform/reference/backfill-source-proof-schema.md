# Backfill Source Proof Schema

`schema_version`: `backfill-source-proof.v1`

This schema defines the minimum fields required before a source family can be
used for canonical raw payload storage, normalized semantic tables, or
NautilusTrader catalog projection.

The source proof is a thin evidence gate. It does not store heavy data; it
points to raw-payload samples, checksums, manifests, license evidence, and parser
proofs under the configured `artifact_root`.

## Source Proof Record

Required fields:

- `source_proof_id`: stable id.
- `source_proof_version`: positive integer.
- `contract_version`: governing backfill table contract version.
- `status`: `pending`, `accepted`, or `rejected`.
- `source_binding`: config-selected source key.
- `venue`: canonical venue key.
- `product_family`: venue/product partition.
- `product_category`: cross-venue category.
- `table_family`: canonical table family.
- `evidence_state`: one of the states in `backfill-table-contract.v1`.
- `fixture_type`: `binary-option` or `perps-spot` for accepted BTE source
  proofs. Legacy/non-current evidence rows may be reported under older fixture
  labels, but they cannot be accepted as canonical BTE input.
- `requested_time_range`: inclusive start and exclusive end in UTC.
- `coverage_time_range`: proven source coverage in UTC, using the same
  inclusive start and exclusive end convention as `requested_time_range`.
- `instrument_universe_id`: manifest id for instruments active during the
  requested window.
- `raw_sample_uri`: portable sample raw-payload pointer under `artifact_root`.
- `raw_sample_hash`: lowercase SHA-256 hex.
- `schema_sample_uri`: parser/schema sample pointer under `artifact_root`.
- `schema_sample_hash`: lowercase SHA-256 hex.
- `license_ref`: license or terms evidence pointer and timestamp.
- `retention_ref`: source retention/freshness evidence pointer and timestamp.
- `nt_mapping_status`: `accepted`, `pending`, `rejected`, or `not_applicable`.
- `fidelity_class`: `L2_REPLAY`, `SNAPSHOT_REPLAY`, `TRADE_REPLAY`,
  `TRADE_BAR_REPLAY`, `METADATA_ONLY`, `SIGNAL_ONLY`, or
  `FORWARD_CAPTURE_PENDING`.
- `forbidden_claims`: claims this source must not support.
- `acceptance_scope`: structured manifest/run summary with `planned_objects`,
  `completed_objects`, `failed_objects`, `skipped_objects`, `accepted_bytes`,
  and `selector_scope_violations`.
- `claim_limits`: list of machine-readable limitation records with `id`,
  `severity`, `claim`, `reason`, and `evidence_ref`.
- `cross_market_components`: optional list of point-in-time component source
  proofs for cross-market signal families; required for `product_category =
  "kimchi-premium"`.
- `gap_policy_id`: required when gaps are tolerated.
- `required_checks`: structured check results.

Accepted records are immutable. A new schema, source, coverage window, parser,
license, or fidelity finding creates a new `source_proof_version` or a new
`source_proof_id` that supersedes the prior record.

Accepted proofs require a backfillable evidence state. `status=accepted` is
valid for a positive canonical backfill claim only when `evidence_state` is
`directly_backfillable` or `owner_archive_backfillable`.
`bounded_or_current_only`, `pending_source_proof`,
`vendor_or_forward_capture_only`, `not_applicable`, and
`excluded_from_current_scope` cannot be accepted for canonical one-year backfill
selection. They can be recorded only as rejected, excluded, or bounded evidence
with explicit `claim_limits`.

Accepted proofs also require the selected source-binding registry row to declare
matching `market_structure_fixture = "binary-option"` or
`market_structure_fixture = "perps-spot"`. Concrete venue/provider values remain
registry data; the `fixture` field in source-binding rows is a data-family label,
not the market-structure proof fixture.

## Required Checks

Every source proof has explicit pass/fail/pending checks:

- `source_access`: endpoint, bucket, archive listing, or local owner sample is
  reachable without exposing credentials.
- `license`: storage and derived-table use are permitted. Evidence may be a
  public license/terms page, vendor or commercial agreement pointer, or recorded
  operator attestation to written approval; it must be durable enough to audit
  after the proof is accepted.
- `schema`: sample parser identifies every required field and type.
- `time_semantics`: event, capture, and availability timestamps are mapped.
- `instrument_universe`: instruments active during the requested window are
  discoverable, including expired or delisted instruments where required.
- `coverage`: source coverage includes the requested time range or is marked
  bounded/current only.
- `granularity`: source fidelity matches the requested table family; no weaker
  aggregate is substituted.
- `completeness`: row counts, page counts, checksums, and gap thresholds are
  documented.
- `nt_mapping`: normalized rows can map to NautilusTrader types or are declared
  signal/metadata only.
- `storage`: raw payload, schema sample, and manifest pointers are under the
  configured `artifact_root`.

Each required check is a structured record with `status`, `evidence_ref`, and
optional `expires_at_utc`. `status` is `passed`, `pending`, `failed`, or
`not_applicable`. `not_applicable` is valid only when the source proof records
the claim limit that makes the check irrelevant. If `expires_at_utc` is present,
it must be greater than or equal to the proof's exclusive
`coverage_time_range.end_utc`; otherwise the evidence is expired for that proof.

For `METADATA_ONLY` or `SIGNAL_ONLY` proofs, the `nt_mapping` check passes only
when `nt_mapping_status = not_applicable` and `claim_limits` forbid
NautilusTrader catalog/backtest replay claims.

All checks must pass before `status=accepted`. Pending or failed checks keep the
proof out of canonical backfill selection.

## Cross-Market Signal Components

Cross-market signals must prove their component sources as point-in-time inputs,
not by joining independent latest values after the fact.

Each `cross_market_components` row contains:

- `role`: source role, not a venue identity.
- `source_binding`: TOML-selected source key for that component.
- `source_proof_id` and `source_proof_version`: accepted component proof.
- `event_time_utc`: component event timestamp.
- `available_at_utc`: when the component value was available to the join.
- `join_time_utc`: timestamp of the signal join.

For `product_category = "kimchi-premium"`, the required roles are
`korean_spot`, `reference_price`, `fx_quote`, and `token_mapping`.
Production code must not name candidate Korean spot venues; those remain
source-binding/evidence values. Component `event_time_utc` and
`available_at_utc` must be less than or equal to `join_time_utc`, and
`join_time_utc` must be inside the proof coverage window.

## Acceptance Scope

Accepted source proofs must carry `acceptance_scope` so broad or failed backfill
runs cannot be promoted by prose-only completeness evidence.

- `planned_objects` must be positive.
- `completed_objects` must be positive.
- `accepted_bytes` must be positive and must cover every selected object that
  becomes canonical backtest input.
- `failed_objects` must be zero.
- `selector_scope_violations` must be zero.
- `planned_objects` must equal
  `completed_objects + failed_objects + skipped_objects`.
- `skipped_objects` greater than zero requires `gap_policy_id`.

## Ingest Manifest Record

Each backfill run writes an ingest manifest before normalized tables are
promoted:

- `ingest_run_id`
- `contract_version`
- `artifact_root`
- `source_proof_ids`
- `requested_time_range`
- `generated_at`
- `producer`
- `write_mode`: `dry_run`, `local_staging`, or `canonical_s3`
- `raw_payload_records`: ids, URIs, hashes, byte counts, and row counts.
- `normalized_table_records`: table family, partition, row count, min/max event
  time, and transform hash.
- `instrument_universe_records`: universe id, product family, count, and source
  proof id.
- `gap_records`: table family, instrument id, start/end, severity, and allowed
  by gap policy.
- `no_overwrite_proof`: create-only or conditional-write evidence.

`canonical_s3` write mode is forbidden until every source proof referenced by
the manifest is accepted.

## Initial Backfill Window

Unless superseded by a user-approved run manifest, the first one-year planning
window is:

- `start_utc = 2025-06-01T00:00:00Z`
- `end_utc = 2026-06-01T00:00:00Z`

The end timestamp is exclusive.
