# Internal Adversarial Review

**Feature**: `specs/026-nt-backed-iv-engine/`
**Review date**: 2026-06-08
**Reviewed working tree**: branch `026-nt-backed-iv-engine`, final committed implementation tree
**Recommendation**: PASS for the internally reviewed local scope

## Findings

No blocking findings remain after the local fixes below.

## Issues Found And Fixed

| Issue | Evidence | Resolution |
|---|---|---|
| Converter-level ingest rejections could bypass source-health recording. Malformed aggregate/custom custom data returned `IngestRejected` before an `IvIngestEvent` existed, and live msgbus handlers only logged the failure. | RED: `cargo test --locked --test bolt_v3_iv_live_integration live_iv_event_bindings_record_malformed_` failed with `ProductNotFound` for both malformed aggregate and custom-IV source-health queries. | Added runtime rejection recording for pre-ingest converter failures using configured source generation and incoming NT event timestamp. Added live msgbus tests proving malformed aggregate/custom custom data updates `InvalidIvValue` reject reason/counts and source-health timestamps. |
| Custom-data backed aggregate/custom-IV raw payloads discarded adapter-specific custom-data fields after extracting configured typed values. | RED: `cargo test --locked --test bolt_v3_iv_live_integration runtime_custom_data_ingest_preserves_original_json_in_raw_payloads` failed because aggregate/custom raw payload variants had no preserved custom-data JSON field. | Added optional `nt_custom_data_json` to aggregate greeks and custom-IV raw payloads, populated it from NT `CustomDataTrait::to_json()`, and kept strategy products typed/provenance-only. |
| Configured `accepted_conventions` was only parsed, not enforced for NT greeks. | RED: `cargo test --locked --test bolt_v3_iv_live_integration runtime_nt_option_greeks_rejects_unaccepted_convention` initially compiled only after the retention test helper was added, then failed behaviorally because Black-Scholes greeks were accepted when the source allowed only price-adjusted conventions. | Runtime source state now carries `accepted_conventions`, rejects unsupported NT option-greeks and nested option-chain greeks conventions, records `UnsupportedConvention` in source health, and happy-path NT fixtures use the canonical `BLACK_SCHOLES` convention name. |
| Option-greeks events with no mark/bid/ask IV basis were accepted as successful no-op indexing. | RED: `cargo test --locked --test bolt_v3_iv_live_integration runtime_nt_option_greeks_rejects_missing_iv_basis` failed before the fix because an event with no IV basis returned `Ok`. | Store indexing now returns `MissingIvBasis` after raw preservation, runtime records typed source health, and retained raw evidence still flows through retention enforcement. |
| Failed runtime indexing could preserve raw payloads without applying profile retention. | RED: `cargo test --locked --test bolt_v3_iv_live_integration runtime_engine_enforces_retention_after_failed_indexing_ingest` first failed to compile because no raw-event count inspection existed for runtime state. | Added `IvQueryStateHandle::raw_event_count()` for evidence and enforced retention after both successful and failed runtime store ingestion. |
| Raw audit age checks used saturating subtraction, allowing `as_of_ns` before event receipt to bypass age limits. | RED: `cargo test --locked --test bolt_v3_iv_store raw_payload_access_rejects_audit_as_of_before_event_receipt` returned `Ok` before the fix. | Raw audit access now rejects requests whose `as_of_ns` precedes `received_ts_ns` with `RetentionMiss`. |
| Duplicate effective NT topics were checked only for option greeks and option chains. | RED: `cargo test --locked --test bolt_v3_iv_config duplicate_custom_data_nt_topics_reject_before_runtime_binding` loaded duplicate aggregate/custom-IV custom-data topics successfully before the fix. | Config validation now rejects duplicate aggregate greeks `aggregate_key` topics and duplicate custom-IV `custom_iv_data_type` topics within a profile. |
| Reload planning produced new generations, but runtime source state could not apply a reloaded IV root. | RED: `cargo test --locked --test bolt_v3_iv_subscription runtime_engine_reload_updates_configured_source_generations` failed to compile because `IvRuntimeEngine` had no reload API. | Added `IvRuntimeEngine::apply_iv_root_reload()` to refresh source configs, policies, retention, derived inputs, and current subscription generations while preserving existing shared query state handles. |
| Existing cloned strategy handles could continue serving old products after a source generation reload or profile removal. | Adversarial review of `IvQueryHandle` freshness checks showed source-health state could override the new generation map, and removed profiles were dropped from the engine map without mutating already-cloned shared state. | Reload now marks removed sources/profiles as `Removed` in the shared query state before dropping engine references, and query freshness checks require the current generation map to match before source health can satisfy a product query. |
| Speckit data model documented the old aggregate selector shape. | `quickstart.md` and production validation required `delta_field`, `gamma_field`, `vega_field`, `theta_field`, and `rho_field`; `data-model.md` omitted them. | Updated `data-model.md` so `SourceAggregateGreeksSelector` includes the five configured greek field mappings. |

## Prior Findings Rechecked

- Custom-data live routing for aggregate greeks and custom IV evidence is present in `src/bolt_v3_live_node.rs`, retained in `BoltV3IvRuntimeEventBindings`, and unsubscribed in `Drop`.
- Happy-path live routing is covered by `tests/bolt_v3_iv_live_integration.rs`.
- Duplicate option-greeks NT topic validation is present in `src/bolt_v3_iv/config.rs` and covered by `tests/bolt_v3_iv_config.rs`.

## Verification

- `cargo test --locked --test bolt_v3_iv_live_integration live_iv_event_bindings_record_malformed_`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_live_integration runtime_custom_data_ingest_preserves_original_json_in_raw_payloads`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_live_integration runtime_nt_option_greeks_rejects_unaccepted_convention`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_live_integration runtime_nt_option_greeks_rejects_missing_iv_basis`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_live_integration runtime_engine_enforces_retention_after_failed_indexing_ingest`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_store raw_payload_access_rejects_audit_as_of_before_event_receipt`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_config duplicate_custom_data_nt_topics_reject_before_runtime_binding`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_subscription runtime_engine_reload_updates_configured_source_generations`: RED before fix, GREEN after fix.
- `cargo test --locked --test bolt_v3_iv_live_integration --test bolt_v3_iv_subscription --test bolt_v3_iv_config --test bolt_v3_iv_store`: PASS, 49 tests.
- `cargo test --locked --test bolt_v3_iv_live_integration --test bolt_v3_iv_ingest`: PASS, 21 tests.
- `cargo test --locked --test bolt_v3_iv_live_integration`: PASS, 17 tests.
- `cargo test --locked --test bolt_v3_iv_subscription`: PASS, 15 tests.
- `cargo fmt --check`: PASS.
- `just source-fence`: PASS.
- `cargo clippy --locked --lib -- -D warnings`: PASS.
- `cargo clippy --locked --bin bolt-v2 -- -D warnings`: PASS.
- Focused IV target bundle: PASS.
- `cargo test --locked bolt_v3_iv`: PASS.
- `cargo test --locked`: PASS.

Current PR verification uses GitHub CI rather than local cargo reruns. PR #611 was green on reviewed head `23004a14a1987215fb440bed6a3128c20591db3a` before the evidence-file update; final head status must be confirmed after the final push.
