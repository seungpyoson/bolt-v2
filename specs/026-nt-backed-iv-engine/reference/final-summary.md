# Final Branch Summary

**Feature**: `specs/026-nt-backed-iv-engine/`
**Branch**: `026-nt-backed-iv-engine`
**Base SHA**: `0c427740d9005c1376d3bf008606a1b61a92fc9c`
**Pull Request**: `#611` (`https://github.com/seungpyoson/bolt-v2/pull/611`)
**Current review basis**: PR head verified through GitHub CI, not local cargo reruns.

The exact final pushed SHA cannot be embedded in the commit that contains this file without changing that SHA. Treat PR #611 as authoritative: use `gh pr view 611 --json headRefOid` and `gh pr checks 611` after the final push to verify the remote branch tip and exact-head CI status.

## Implemented Scope

- NT IV/options capability discovery and classified capability ledger.
- Typed IV profile/source config and root `[iv]` config parsing.
- NT-backed subscription planning for option greeks, option chains, aggregate greeks, and custom IV evidence.
- Live msgbus event routing for NT option greeks, option-chain slices, aggregate greeks custom data, and custom IV evidence custom data into runtime-backed strategy query handles.
- Runtime fail-closed checks for configured NT greeks conventions, missing IV basis, non-finite strategy-visible numeric fields, stale/reloaded subscription generations, malformed custom data, and audit retention windows.
- Raw event preservation, including serialized NT custom-data JSON for custom-data backed sources, audit-only raw access, and strategy-safe indexed IV products.
- IV points, greeks points, smiles, surfaces, aggregate greeks, custom IV evidence, source health, projected scalar IV, and derived IV query products.
- Typed projection policy selectors with Rust-validated TOML values, all-strikes smile projection, fail-closed single-strike interpolation fallback/rejection, quorum fallback routing, accepted-candidate fallback provenance, per-source smile interpolation before quorum, selector-scoped product candidate selection, current-generation product filtering, helper-output config validation, and read-guard based non-derived query execution.
- Rust validation for policy and derived-input source references, including interpolation/fallback/quorum `eligible_sources`, reciprocal helper/input-policy refs, derived-input `profile_source_ref` source-selector pairs, non-empty and internally consistent derived-input source-kind allowlists, and duplicate derived-input field policies.
- Per-strategy TOML authorization entries so two registered strategy instances in the same IV profile can receive different selector/source scopes through the runtime-backed registry.
- NT helper-backed derived IV through `nautilus_model::data::imply_vol_and_greeks`.
- Strategy query handle registration through `StrategyRegistrationContext::iv_query_handles`.
- Live-node IV startup/stop lifecycle planning through `IvEngineLifecyclePlan` and `plan_iv_engine_lifecycle`, plus IV reload planning and runtime IV root reload state updates for existing query handles. This PR does not introduce a production config hot-reload trigger in the live-node runner.
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
- Historical and latest local RED/GREEN cargo/source-fence evidence remain recorded in `implementation-ledger.md` and `internal-review.md`; current exact-head verification for the PR is GitHub CI after the final push.

## Review Status

- Internal review: complete after the 2026-06-10 review fixes; no blocking findings remain in `reference/internal-review.md`.
- Blocking findings: none remaining from the internal adversarial review.
- External/PR review: CodeQL and Gemini review threads on PR #611 are replied to and resolved. CodeQL is green on the current PR checks.
- Open overlap PRs/issues: none found to close for the IV/options overlap search; #158, #488, and #493 remain open because none were fully ported.

## Blocking Scope Remaining

- No blocking scope remains from the internal adversarial review.

## Residual Risk

- Real venue-backed live NT market-data behavior is not exercised locally; coverage is through typed subscription/lifecycle plans, raw ingest/store tests, query tests, config tests, and source-fence.
- Because this summary is committed into the branch, exact final head and CI status must always be checked from PR #611 after the final push.
