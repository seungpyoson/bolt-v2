# Final Branch Summary

**Feature**: `specs/026-nt-backed-iv-engine/`
**Branch**: `026-nt-backed-iv-engine`
**Base SHA**: `c1b1f7b49414008a11af11da24ebc49762debf54`
**Final local implementation**: committed on branch `026-nt-backed-iv-engine`

The exact final pushed SHA cannot be embedded in the commit that contains this file without changing that SHA. Record the final remote branch tip from `git ls-remote origin 026-nt-backed-iv-engine` after push.

## Implemented Scope

- NT IV/options capability discovery and classified capability ledger.
- Typed IV profile/source config and root `[iv]` config parsing.
- NT-backed subscription planning for option greeks, option chains, aggregate greeks, and custom IV evidence.
- Live msgbus event routing for NT option greeks, option-chain slices, aggregate greeks custom data, and custom IV evidence custom data into runtime-backed strategy query handles.
- Runtime fail-closed checks for configured NT greeks conventions, missing IV basis, stale/reloaded subscription generations, malformed custom data, and audit retention windows.
- Raw event preservation, including serialized NT custom-data JSON for custom-data backed sources, audit-only raw access, and strategy-safe indexed IV products.
- IV points, greeks points, smiles, surfaces, aggregate greeks, custom IV evidence, source health, projected scalar IV, and derived IV query products.
- NT helper-backed derived IV through `nautilus_model::data::imply_vol_and_greeks`.
- Strategy query handle registration through `StrategyRegistrationContext::iv_query_handles`.
- Live-node IV lifecycle planning through `IvEngineLifecyclePlan` and `plan_iv_engine_lifecycle`, plus runtime IV root reload state updates and removed-source invalidation for existing query handles.
- IV source-fence checks wired through `just source-fence`.

## NT APIs Used

- Pinned NT Cargo evidence from `Cargo.toml` and `Cargo.lock`.
- NT option-greeks, option-chain, aggregate-greeks, and custom-data subscription surfaces discovered from the pinned NT checkout.
- `nautilus_model::data::imply_vol_and_greeks` for helper-backed derived IV.
- `nautilus_model::data::black_scholes_greeks` in tests to generate deterministic helper inputs.

## Verification

- `cargo test --locked bolt_v3_iv`: PASS
- `cargo test --locked --test bolt_v3_iv_capability --test bolt_v3_iv_config --test bolt_v3_iv_live_integration --test bolt_v3_iv_subscription --test bolt_v3_iv_ingest --test bolt_v3_iv_store --test bolt_v3_iv_query --test bolt_v3_iv_policy --test bolt_v3_iv_derive --test bolt_v3_iv_source_fence --test config_parsing`: PASS
- `cargo fmt --check`: PASS
- `cargo clippy --locked --lib -- -D warnings`: PASS
- `cargo clippy --locked --bin bolt-v2 -- -D warnings`: PASS
- `just source-fence`: PASS
- `cargo test --locked`: PASS

## Review Status

- Internal review: complete after local fixes; no blocking findings remain in `reference/internal-review.md`.
- Blocking findings: none remaining from the internal adversarial review.
- External review: not requested because there is no PR and no exact-head CI run.
- Open overlap PRs/issues: none found to close for the IV/options overlap search; #158, #488, and #493 remain open because none were fully ported.

## Blocking Scope Remaining

- No blocking scope remains from the internal adversarial review.

## Residual Risk

- No PR CI has run for the exact final head.
- Real venue-backed live NT market-data behavior is not exercised locally; coverage is through typed subscription/lifecycle plans, raw ingest/store tests, query tests, config tests, and source-fence.
