# Final Branch Summary

**Feature**: `specs/026-nt-backed-iv-engine/`
**Branch**: `026-nt-backed-iv-engine`
**Base SHA**: `c1b1f7b49414008a11af11da24ebc49762debf54`
**Pull Request**: `#611` (`https://github.com/seungpyoson/bolt-v2/pull/611`)
**Current review basis**: PR head verified through GitHub CI, not local cargo reruns.

The exact final pushed SHA cannot be embedded in the commit that contains this file without changing that SHA. Treat PR #611 as authoritative: use `gh pr view 611 --json headRefOid` and `gh pr checks 611` after the final push to verify the remote branch tip and exact-head CI status.

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

- PR #611 GitHub CI is the current verification source for the branch head.
- Passing CI gates included `gate`, `test`, `nextest shard 1 of 4`, `nextest shard 2 of 4`, `nextest shard 3 of 4`, `nextest shard 4 of 4`, `nextest archive`, `clippy`, `deny`, `build`, `check-aarch64`, `source-fence`, `fmt-check`, `detector`, `actionlint`, `CodeQL`, `Analyze (rust)`, `Analyze (actions)`, `bvs-detect`, `bvs-fmt`, `bvs-clippy`, `bvs-test`, and `backtester-gate`.
- Expected non-blocking skips: `deploy` and `same-sha-main-evidence`.
- Historical local RED/GREEN and local cargo/source-fence evidence remain recorded in `implementation-ledger.md` and `internal-review.md`; current verification for the PR is GitHub CI.

## Review Status

- Internal review: complete after the review fixes; no blocking findings remain in `reference/internal-review.md`.
- Blocking findings: none remaining from the internal adversarial review.
- External/PR review: CodeQL and Gemini review threads on PR #611 are replied to and resolved. CodeQL is green on the current PR checks.
- Open overlap PRs/issues: none found to close for the IV/options overlap search; #158, #488, and #493 remain open because none were fully ported.

## Blocking Scope Remaining

- No blocking scope remains from the internal adversarial review.

## Residual Risk

- Real venue-backed live NT market-data behavior is not exercised locally; coverage is through typed subscription/lifecycle plans, raw ingest/store tests, query tests, config tests, and source-fence.
- Because this summary is committed into the branch, exact final head and CI status must always be checked from PR #611 after the final push.
