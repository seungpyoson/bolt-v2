# Probability Typed-Value Pilot Cost

Date: 2026-07-02 KST
Branch: `feature/probability-typed-value-pilot`
Worktree: `.worktrees/probability-typed-value-pilot`
Tracker: `bolt-v2-h7z`
Full Rust proof head: `c360acb5dbfe77b0eac8fc1cc3cbb33940cfc2c9`

## Scope

This pilot typed the taker and fair-value probability compute surface only. Public wire structs and log/evidence payloads remain `f64` or `String` boundaries, and maker/quoting scope stayed limited to return-type blast radius from the shared fair-probability binding.

## Anchor Drift

The approved spec anchored at commit `172fda98`. The branch merge base for the
pilot was `ee24951fee5a818e97e754ceb7749c37c68fa2a0`; the full Rust proof head was
`c360acb5dbfe77b0eac8fc1cc3cbb33940cfc2c9`.

Anchor searches covered `UNIT_F64 -`, `clamp_probability`, `sanitize_probability`, `fair_probability_up_for_family`, `price_agreement_corr`, `price_gap_probability`, entry/exit log fields, and maker reference-fair-value references. Drift was line-number only for the in-scope taker/fair-value probability sites. Non-probability `clamp_probability` scalar sites, including taker jitter and sizing scale sites, stayed raw `f64`.

## Touched Files And Size

Final tracked diff from `git diff --stat $(git merge-base main HEAD)`:
25 files, +961/-287. New probability verifier scripts add 421 lines, justfile
wiring adds 4 lines, and this measurement artifact adds 107 lines. The
Probability module itself adds 111 lines, including constructor, named arithmetic,
doctest, and unit-test coverage. Runtime/call-site churn across production Rust
is +241/-259; Rust test and fixture churn is +74/-25; governance allowlist drift
is +3/-3.

Exact files touched:

- `docs/bolt-v3/probability-pilot-cost.md`
- `justfile`
- `scripts/test_verify_probability_typed_pilot.py`
- `scripts/verify_probability_typed_pilot.py`
- `specs/711-capital-admission-rename/misnomer-allowlist.txt`
- `src/bolt_v3_binary_outcome_edge.rs`
- `src/bolt_v3_decision_evidence.rs`
- `src/bolt_v3_fair_value_pricing.rs`
- `src/bolt_v3_market_families/hyperliquid_instrument.rs`
- `src/bolt_v3_market_families/mod.rs`
- `src/bolt_v3_market_families/outcome_group.rs`
- `src/bolt_v3_market_families/static_binary_event.rs`
- `src/bolt_v3_market_families/updown.rs`
- `src/bolt_v3_numeric.rs`
- `src/bolt_v3_taker_pricing.rs`
- `src/bolt_v3_taker_updown_signal.rs`
- `src/strategies/binary_oracle_edge_taker/entry_decision.rs`
- `src/strategies/binary_oracle_edge_taker/mod.rs`
- `src/strategies/binary_oracle_edge_taker/tests/pricing.rs`
- `src/strategies/binary_oracle_edge_taker/tests/shared_fixture.rs`
- `src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs`
- `tests/bolt_v3_binary_oracle_maker_runtime.rs`
- `tests/bolt_v3_decision_evidence.rs`
- `tests/bolt_v3_maker_runtime_quote.rs`
- `tests/bolt_v3_static_binary_event_reference_price.rs`

## Verification Evidence

- `cargo fmt`: pass.
- `just fmt-check`: pass.
- `just verify-probability-typed-pilot`: pass.
- `just verify-bolt-v3-schema-current`: pass.
- `git diff --check`: pass.
- `just source-fence-static`: first run failed on stale `src/bolt_v3_decision_evidence.rs` line-number allowlist entries caused by the helper insertion; the allowlist coordinates were updated and the rerun passed.
- `rg -n 'Probability\(' src tests scripts docs`: no tuple construction hits.
- Forbidden derive/conversion sweep: no `Eq`, `Hash`, `Serialize`,
  `Deserialize`, `From<f64>`, or `Into<Probability>` implementation on
  `Probability`.
- `just verify-remote`: exact-head full CI passed for
  `c360acb5dbfe77b0eac8fc1cc3cbb33940cfc2c9`, workflow run
  `28537460833` (`CI [dispatch:full]`), completed success on 2026-07-02
  03:13 KST.

Compile-heavy Rust proof was remote-only per repo policy; no local cargo build,
clippy, or nextest run was used as proof.

## Review Rounds

Local implementation/static-verifier loop: 1 substantive loop. Remote CI feedback
rounds after opening the draft PR: 4 fix pushes after the initial implementation
before exact-head full CI was green. D/Q decisions did not change mid-flight.
External required-review round at artifact update time: not yet requested.

Implementation timeline from branch commits and final green run: first
implementation commit at 2026-07-02 00:59 KST; exact-head full CI green at
2026-07-02 03:13 KST. Implementation-to-green elapsed time: about 2h14m.

## Friction Log

- The `Probability` type itself was small, but the shared market-family return type touched maker-adjacent compile surfaces even though maker behavior stayed out of scope.
- The decision-evidence helper insertion moved existing legacy misnomer allowlist line numbers, which required a source-fence metadata update.
- New Python verifier scripts needed lane-governor entrypoint wiring before `source-fence-static` would pass.
- The static gate is broad and slow enough that reruns should be batched after local verifier changes.
- Remote CI exposed test-only typed-boundary misses in integration targets that
  local noncompile gates could not compile-check.
- A numeric unit test used decimal literals that are not exactly representable as
  binary floating values; exact-representable fixtures fixed the assertion without
  changing the `Probability` implementation.

## Caveat

This artifact measures the pilot implementation, static evidence, and exact-head
remote CI feedback. It is not merge readiness by itself; required native review
and ready-PR/merge-queue proof remain required before merging under repository
governance.
