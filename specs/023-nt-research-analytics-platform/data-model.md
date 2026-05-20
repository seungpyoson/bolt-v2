# Data Model: NT-First Research Analytics Platform

All entities here are planning contracts. They do not authorize runtime code until
their evidence rows are `SOURCE_PROVEN` or explicitly accepted as `USER_ASSUMPTION`.

## EvidenceClaim

- `id`: Stable ledger id, e.g. `E-003`.
- `claim`: One assertion that may appear in specs, plans, tasks, or issue bodies.
- `status`: `SOURCE_PROVEN`, `USER_ASSUMPTION`, `GAP`, `DECISION_NEEDED`, or
  an explicit combined status when one row carries mixed facts.
- `evidence`: Exact file/doc/issue/source reference.
- `implication`: What the plan may do because of the claim.
- `next_proof`: Concrete proof required before implementation or issue closure.

## NtCapabilityProof

- `surface`: Backtest, catalog, adapter, report, snapshot, data feed, execution, or settlement.
- `venue`: TOML/registry-selected venue key. Examples include Polymarket,
  Hyperliquid HIP-4, Kalshi, selected perpetual-futures venue, or cross-venue,
  but the model is not a closed venue enum.
- `nt_pointer`: Exact NT commit/tag used for proof.
- `bolt_pointer`: Exact Bolt commit used for proof.
- `status`: `SOURCE_PROVEN`, `USER_ASSUMPTION`, `GAP`, or `DECISION_NEEDED`.
- `api_refs`: NT Rust/Python/docs references.
- `compile_status`: not-run, passed, failed, or blocked.
- `behavior_status`: not-run, source-only, dry-run, testnet, no-submit, or live.
- `residual_gap`: Remaining unsupported surface, if any.

## ProviderMode

- `provider`: TOML/registry-selected provider key. Examples include Tardis,
  Telonex, Goldsky, official archive/API, or forward capture, but the model is
  not a closed provider enum.
- `venues`: Venue/data families covered.
- `data_classes`: Order book deltas, depth snapshots, trades, quotes, bars, instruments, fills, account state, settlements, on-chain fills, or metadata.
- `cost_components`: Subscription, AWS storage, compute, transfer, logs, query, dashboard, and reserve.
- `license_scope`: Personal, commercial, enterprise, public, unknown, or waived.
- `freshness`: Real-time, daily, monthly, archive, or forward-capture only.
- `fidelity_class`: Link to `DataFidelityClass`.
- `selected`: true only after cost and fidelity gates pass.

## DataFidelityClass

- `class`: `L2_REPLAY`, `TRADE_BAR_REPLAY`, `SIGNAL_ONLY`, or `FORWARD_CAPTURE_PENDING`.
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
- `source_records`: Raw evidence records consumed.
- `nt_pointer`: Exact NT commit used for conversion.
- `catalog_path`: NT catalog path.
- `data_type`: NT data class.
- `instrument_ids`: NT instrument ids included.
- `transform_hash`: Transform/config hash.
- `fidelity_class`: Backtest-claim class.
- `validation_status`: not-run, passed, failed, or partial.

## ResearchRunManifest

- `manifest_id`: Stable run id.
- `nt_config_mapping`: Fields that map directly to NT config.
- `bolt_metadata`: Orchestration-only fields, not domain semantics.
- `venue_binding_key`: TOML-selected registry key for the venue/product binding.
- `provider_binding_key`: TOML-selected registry key for historical/source provider binding.
- `strategy_config_ref`: TOML or NT-native config reference.
- `catalog_projection`: Input projection id.
- `time_range`: Start/end.
- `fill_model`: NT fill model or explicit lower-fidelity label.
- `output_path`: Report/result path.
- `lineage_hash`: Hash over manifest, strategy config, catalog projection, and NT pointer.

## ResearchResult

- `run_manifest`: Manifest id.
- `reports`: NT report outputs used.
- `metrics`: Derived analysis metrics.
- `fidelity_class`: Copied from input projection.
- `claim_limits`: Text shown with result so lower-fidelity runs cannot be sold as execution-quality.
- `source_hashes`: Raw/catalog/run hashes.

## DashboardReadModel

- `source_contract`: Dashboard source contract id.
- `source_events`: NT events, reports, snapshots, or derived analytics tables.
- `freshness`: Last seen timestamp and stale threshold.
- `display_scope`: Current trades, positions, PnL, exposure, data health, strategy state, or outlook.
- `mutation_capability`: Must be `none`.
- `gap_label`: Missing source such as `portfolio_snapshot_not_captured`.

## IssueSlice

- `issue_title`: Proposed title.
- `existing_issue_relation`: updates, depends-on, blocks, duplicates, or new.
- `evidence_rows`: Ledger rows justifying the slice.
- `accepted_scope`: Exact work included.
- `residual_scope`: Explicitly out of scope or tracked elsewhere.
- `acceptance_evidence`: Files, commands, reports, or issue comments required.
- `review_packet`: Source packet for external review if requested.
