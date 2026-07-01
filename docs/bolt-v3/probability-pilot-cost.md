# Probability Typed-Value Pilot Cost

Date: 2026-07-02 KST
Branch: `feature/probability-typed-value-pilot`
Worktree: `.worktrees/probability-typed-value-pilot`
Tracker: `bolt-v2-h7z`

## Scope

This pilot typed the taker and fair-value probability compute surface only. Public wire structs and log/evidence payloads remain `f64` or `String` boundaries, and maker/quoting scope stayed limited to return-type blast radius from the shared fair-probability binding.

## Anchor Drift

The approved spec anchored at commit `172fda98`. The pilot was re-checked at `ee24951fee5a818e97e754ceb7749c37c68fa2a0`.

Anchor searches covered `UNIT_F64 -`, `clamp_probability`, `sanitize_probability`, `fair_probability_up_for_family`, `price_agreement_corr`, `price_gap_probability`, entry/exit log fields, and maker reference-fair-value references. Drift was line-number only for the in-scope taker/fair-value probability sites. Non-probability `clamp_probability` scalar sites, including taker jitter and sizing scale sites, stayed raw `f64`.

## Touched Files And Size

Pre-artifact tracked diff: 19 files, +426/-283. New probability verifier scripts add 419 lines. The full local pre-artifact edit set was 21 files, +845/-283, before this measurement file.

Primary files touched:

- `src/bolt_v3_numeric.rs`
- `src/bolt_v3_market_families/{mod.rs,updown.rs,static_binary_event.rs,outcome_group.rs,hyperliquid_instrument.rs}`
- `src/bolt_v3_fair_value_pricing.rs`
- `src/bolt_v3_taker_pricing.rs`
- `src/bolt_v3_taker_updown_signal.rs`
- `src/bolt_v3_binary_outcome_edge.rs`
- `src/strategies/binary_oracle_edge_taker/{mod.rs,entry_decision.rs}`
- `src/strategies/binary_oracle_edge_taker/tests/{pricing.rs,shared_fixture.rs,source_evidence.rs}`
- `src/bolt_v3_decision_evidence.rs`
- `tests/bolt_v3_decision_evidence.rs`
- `scripts/{verify_probability_typed_pilot.py,test_verify_probability_typed_pilot.py}`
- `justfile`
- `specs/711-capital-admission-rename/misnomer-allowlist.txt`

## Verification Evidence

- `cargo fmt`: pass.
- `just fmt-check`: pass.
- `just verify-probability-typed-pilot`: pass.
- `just verify-bolt-v3-schema-current`: pass.
- `git diff --check`: pass.
- `just source-fence-static`: first run failed on stale `src/bolt_v3_decision_evidence.rs` line-number allowlist entries caused by the helper insertion; the allowlist coordinates were updated and the rerun passed.

Remote compile/test proof was not run locally. Per the repo remote-first Rust policy, compile-heavy Rust proof remains an exact-head PR CI step.

## Review Rounds

Local implementation review/static-verifier loop: 1 substantive loop. External review rounds at artifact time: 0.

## Friction Log

- The `Probability` type itself was small, but the shared market-family return type touched maker-adjacent compile surfaces even though maker behavior stayed out of scope.
- The decision-evidence helper insertion moved existing legacy misnomer allowlist line numbers, which required a source-fence metadata update.
- New Python verifier scripts needed lane-governor entrypoint wiring before `source-fence-static` would pass.
- The static gate is broad and slow enough that reruns should be batched after local verifier changes.

## Caveat

This artifact measures the local pilot implementation and static evidence. It is not merge readiness by itself; exact-head remote CI and required review remain required before completion under repository governance.
