# Data Model: World Cup Production Profit Gate

## EventMarketSourceProof

Source-owned eligibility record for one event market.

**Fields**

- `proof_id`
- `event_family`
- `competition_id`
- `market_family`
- `venue_id`
- `account_scope`
- `product_surface`
- `official_event_source_url`
- `official_event_source_sha256`
- `official_event_retrieved_at_unix_seconds`
- `official_event_expires_at_unix_seconds`
- `venue_market_terms_url`
- `venue_market_terms_sha256`
- `venue_market_terms_retrieved_at_unix_seconds`
- `resolution_rule`
- `void_rule`
- `postponement_rule`
- `abandonment_rule`
- `settlement_rule`
- `jurisdiction_availability_status`
- `jurisdiction_availability_source_url`
- `jurisdiction_availability_sha256`
- `config_checksum`
- `commit_sha`

**Validation Rules**

- All URLs must be non-empty and accompanied by sha256.
- Expiry must be positive and later than retrieval.
- Resolution fields must be explicit; market-name inference is rejected.
- Venue terms and official event rules must not conflict.
- Jurisdiction availability must be accepted for the venue/account/product surface.

## ProviderCapabilityProof

Provider-neutral record of what a data provider can actually supply.

**Fields**

- `provider_id`
- `proof_id`
- `provider_terms_url`
- `provider_terms_sha256`
- `plan_name`
- `plan_entitlement_source_url`
- `transport_class`
- `stream_protocol`
- `update_semantics`
- `requires_rest_refresh`
- `supported_leagues`
- `supported_markets`
- `supported_books`
- `historical_tick_support`
- `order_book_depth_support`
- `latency_class`
- `rate_limit_policy`
- `license_scope`
- `retrieved_at_unix_seconds`
- `expires_at_unix_seconds`
- `commit_sha`

**Validation Rules**

- Provider name alone is never a capability.
- Direct source classification requires direct provider proof.
- Aggregator-sourced bookmaker odds must keep the aggregator label.
- Expired plan entitlement rejects the provider role.

## ReferenceQuorumPolicy

TOML-owned policy for reference data roles.

**Fields**

- `policy_id`
- `market_family`
- `primary_roles`
- `backup_roles`
- `veto_roles`
- `max_provider_staleness_milliseconds`
- `min_accepted_primary_count`
- `min_accepted_backup_count`
- `veto_on_conflict`
- `quorum_loss_action`

**Validation Rules**

- At least one accepted primary role is required.
- Stale providers do not count toward quorum.
- A configured veto conflict blocks new order intent.
- Quorum loss fails closed and emits an operator-visible reason.

## ProfitEvidenceSession

NT-backed session that evaluates whether observed edge survived execution costs.

**Fields**

- `session_id`
- `market_proof_hash`
- `provider_capability_hashes`
- `quorum_policy_hash`
- `nt_catalog_path_hash`
- `capture_started_at_unix_seconds`
- `capture_ended_at_unix_seconds`
- `fidelity_class`
- `candidate_count`
- `no_trade_count`
- `executable_edge_decision_hash`
- `order_book_depth_evidence_hash`
- `fee_evidence_hash`
- `fill_evidence_hash`
- `cancel_evidence_hash`
- `markout_evidence_hash`
- `settlement_evidence_hash`
- `profit_summary_hash`
- `threshold_policy_hash`
- `accepted`
- `rejection_reasons`

**Validation Rules**

- Evidence hashes must bind to the exact market proof and provider proof.
- Lower-fidelity sessions cannot support capital-scale promotion.
- Positive edge without fill/markout/settlement evidence cannot advance.
- Missing book depth or fee evidence rejects executable-profit claims.

## ExecutionMarkoutRecord

Per decision/fill markout record used by `ProfitEvidenceSession`.

**Fields**

- `decision_id`
- `instrument_id`
- `side`
- `order_shape`
- `quoted_price`
- `exact_size_vwap_price`
- `fee_bps`
- `submitted_quantity`
- `filled_quantity`
- `fill_price`
- `markout_horizons_milliseconds`
- `markout_prices`
- `adverse_selection_score`
- `source_freshness_milliseconds`
- `rejection_or_fill_reason`

**Validation Rules**

- All timestamps and horizons must be monotonic.
- Unfilled quotes still produce cancel/no-fill evidence.
- Markout records must bind to the same source-proofed market.

## ProductionPromotionPackage

Disabled package emitted after evidence passes.

**Fields**

- `package_id`
- `source_proof_hash`
- `provider_capability_hashes`
- `profit_evidence_session_hash`
- `generated_config_path`
- `generated_config_sha256`
- `enabled`
- `commit_sha`
- `config_checksum`
- `operator_review_status`

**Validation Rules**

- `enabled` must be false at creation.
- Package cannot mutate secrets, SSM, venue state, orders, or funds.
- Package cannot advance without accepted source and profit evidence.

## LiveEnablementGate

Exact-head gate before controlled-connect/capital-probe/live-capital progression.

**Fields**

- `gate_id`
- `promotion_package_hash`
- `exact_head_commit_sha`
- `ci_status_hash`
- `source_fence_status_hash`
- `controlled_connect_report_hash`
- `capital_probe_proof_hash`
- `operator_approval_hash`
- `legal_geography_proof_hash`
- `venue_account_product_hash`
- `state`
- `rejection_reasons`

**Validation Rules**

- Missing exact-head CI rejects the gate.
- Missing or stale controlled-connect evidence rejects capital-probe readiness.
- Canary proof is scoped to one venue/account/product/market-family/config hash.
- Operator approval must be current, explicit, and bound to the same package.
