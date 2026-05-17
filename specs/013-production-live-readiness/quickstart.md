# Quickstart: Production Live Readiness

This quickstart validates the Issue #369 readiness-definition slice. It is not approval to run live capital.

## Local Verification

```bash
cargo test --test bolt_v3_production_readiness_contract -- --nocapture
cargo test --test bolt_v3_no_submit_readiness -- --nocapture
cargo test --test bolt_v3_tiny_canary_preconditions -- --nocapture
cargo fmt --check
git diff --check
```

## Reviewer Checklist

- Read `docs/bolt-v3/2026-05-18-production-readiness-contract.md`.
- Confirm the contract defines tiny-canary, staged live, and production live readiness.
- Confirm the contract blocks production-grade claims without evidence or explicit waiver.
- Confirm `docs/bolt-v3/2026-04-25-bolt-v3-contract-ledger.md` links the contract.
- Confirm `docs/bolt-v3/2026-04-28-source-grounded-status-map.md` row 48 links the contract.
- Confirm `tests/bolt_v3_production_readiness_contract.rs` protects the required artifact surface.

## Scope Boundary

This slice defines and tests the production-readiness contract. It does not implement staged live operation, production deployment, or any live submit path.
