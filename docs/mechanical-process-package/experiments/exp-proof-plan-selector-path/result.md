# Proof Plan Adequacy Experiment Result

## Hypothesis

The process should force the real selector-path late blocker classes into explicit pre-review claims and falsifiers, instead of leaving them to external review.

## Result

Partial pass.

### What the proof-plan gate can catch

The proof-plan gate is strong enough for:

1. single schema-boundary behavior across ruleset mode and legacy event_slugs mode
2. ruleset-mode typo rejection
3. fail-closed legacy empty-event-slugs behavior

Those blocker classes map cleanly to explicit claims and explicit falsifiers.

Direct local evidence collected:

- `cargo test --lib ruleset_mode_rejects_unknown_field_in_polymarket_data_client_config -- --nocapture`
- `cargo test --lib legacy_event_slugs_config_builds_client_without_rulesets -- --nocapture`
- `cargo test --lib build_data_client_rejects_empty_event_slugs_without_rulesets -- --nocapture`
- `cargo test --test polymarket_bootstrap ruleset_mode_rejects_legacy_event_slugs_during_bootstrap -- --nocapture`
- `cargo test --test polymarket_catalog prefix_selector_slug_fetch_respects_gamma_event_fetch_max_concurrent -- --nocapture`

All five passed on the current local tree.

### What the proof-plan gate cannot own alone

The proof-plan gate is not the right owner for:

1. stale review-target artifacts
2. clean-base vs follow-up-slice decisions for unbounded slug fan-out

Those require:

- `review_target.toml` for stale-diff filtering
- `issue_contract.toml` plus `finding_ledger.toml` for explicit defer-vs-fix handling

## Verdict

This is the useful result:

proof-plan is necessary but not sufficient.

That is not a weakness in the process.
It is the correct decomposition.

The experiment shows that the process can assign late blocker classes to the right gate instead of pretending one gate solves everything.

## Archived Plan Verdict

The archived selector-path plan at [.omx/context/issue-175-partial-archive-2026-04-16/2026-04-16-issue-175-selector-discovery.md](/Users/spson/Projects/Claude/bolt-v2/.omx/context/issue-175-partial-archive-2026-04-16/2026-04-16-issue-175-selector-discovery.md:1) fails this adequacy test.

Why:

- it required legacy rejection in ruleset mode
- it did not require explicit proof of unknown-field rejection at the single schema boundary
- it did not require positive legacy-mode compatibility proof
- it did not name slug-fetch concurrency as an explicit proof or tracked follow-up class
