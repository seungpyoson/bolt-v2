# Quickstart: Fee-Provider Binding Decoupling

## Pre-Implementation Gates

1. Confirm branch and head:

```bash
git status --short --branch
git rev-parse HEAD
```

2. Review planning artifacts:

```bash
sed -n '1,220p' specs/453-fee-provider-decoupling/spec.md
sed -n '1,260p' specs/453-fee-provider-decoupling/research.md
sed -n '1,220p' specs/453-fee-provider-decoupling/tasks.md
```

3. Get external source-proven plan review before implementation.

## TDD Loop

Run the Phase 2 red confirmations before provider resolver production edits:

```bash
cargo test --test bolt_v3_strategy_registration fee_provider_source_fence_blocks_concrete_provider_in_shared_layers -- --nocapture
cargo test --lib fee_provider_resolution_uses_provider_binding_registry -- --nocapture
cargo test --lib fee_provider_resolution_rejects_missing_execution_client_id -- --nocapture
cargo test --lib fee_provider_resolution_rejects_unsupported_provider_kind -- --nocapture
cargo test --lib fee_provider_resolution_rejects_provider_without_fee_binding -- --nocapture
cargo test --lib fee_provider_resolution_reports_provider_config_parse_failure -- --nocapture
cargo test --lib fee_provider_resolution_rejects_invalid_secret_binding -- --nocapture
cargo test --lib fee_provider_resolution_reports_provider_client_construction_failure -- --nocapture
cargo test --lib fee_provider_resolution_error_display_debug_redacts_sentinel_secret -- --nocapture
cargo test --lib fee_provider_resolution_redacts_provider_build_secret_errors -- --nocapture
cargo test --test bolt_v3_strategy_registration fee_provider_resolution_does_not_warm_during_registration -- --nocapture
```

Then implement minimal green behavior and rerun the focused test.

Run the runtime registration red confirmation before archetype production edits:

```bash
cargo test --test bolt_v3_strategy_registration binary_oracle_registration_resolves_fee_provider_through_provider_boundary -- --nocapture
```

## Required Verification After Implementation

```bash
cargo test --test bolt_v3_strategy_registration -- --nocapture
cargo test --test bolt_v3_strategy_registration binary_oracle_registration_resolves_fee_provider_through_provider_boundary -- --nocapture
cargo test --lib fee_provider_resolution_uses_provider_binding_registry -- --nocapture
cargo test --lib fee_provider_resolution_rejects_missing_execution_client_id -- --nocapture
cargo test --lib fee_provider_resolution_rejects_unsupported_provider_kind -- --nocapture
cargo test --lib fee_provider_resolution_rejects_provider_without_fee_binding -- --nocapture
cargo test --lib fee_provider_resolution_reports_provider_config_parse_failure -- --nocapture
cargo test --lib fee_provider_resolution_rejects_invalid_secret_binding -- --nocapture
cargo test --lib fee_provider_resolution_reports_provider_client_construction_failure -- --nocapture
cargo test --lib fee_provider_resolution_error_display_debug_redacts_sentinel_secret -- --nocapture
cargo test --lib fee_provider_resolution_redacts_provider_build_secret_errors -- --nocapture
cargo test --test bolt_v3_strategy_registration fee_provider_resolution_does_not_warm_during_registration -- --nocapture
cargo test --test bolt_v3_strategy_registration fee_provider_source_fence_blocks_concrete_provider_in_shared_layers -- --nocapture
cargo test --lib fee_provider_ -- --nocapture
cargo fmt -- --check
just clippy
```

Run broader Rust verification before PR:

```bash
cargo test --locked
```

## Proof Boundary

Passing local tests proves registration/provider-source behavior only. It does not prove new venue readiness, live exchange execution, or #451 generic admission/submission wrapper completion.
