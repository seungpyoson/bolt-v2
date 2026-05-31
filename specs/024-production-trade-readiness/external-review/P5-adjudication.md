# P5 Adjudication — Market-Family / Instrument-Filter (PR #480)

HEAD `1f6ee056`. 6 external models; every finding re-verified vs HEAD bytes (and the pinned
NT rev `6e059dc` for adapter semantics). Verdict: **HAS-LIVE-MONEY-CRITICAL (conditional)** —
driven solely by P5-5 (venue-unscoped selection from the global cache); not reachable under the
current single-venue config but a genuine missing fail-closed invariant. Everything else is
HARDENING.

Anchors use function name + file (line numbers approximate; re-locate by name + re-verify).
**Every fix must preserve or TIGHTEN fail-closed behavior — never loosen a guard.**

## CONFIRMED — actionable

- **P5-5 (LIVE-MONEY, conditional) — DEFERRED to multi-venue workstream (operator decision 2026-05-31).**
  NOT reachable in the current single-venue (Polymarket-only) config — a wrong-venue selection
  cannot occur while only one venue's instruments are in the NT cache, and the existing
  `up_venue == down_venue` self-consistency guard (`selected_market_requirement`) stays. The clean
  fix needs the execution venue (`root.clients[execution_client_id].venue`) threaded into the
  strategy build context (~8 `StrategyBuildContext::new` sites + venue-resolution wiring) — broad
  surgery on the live strategy that is the wrong risk/reward immediately before the real-money
  canary, for an invariant that cannot fire single-venue. TRACK as a fail-closed invariant for the
  multi-venue work; add a loud code comment at the selection site; revisit when a second venue's
  instruments can coexist in the cache. Original analysis retained below for that future work.
  `src/bolt_v3_market_families/updown.rs`
  (`updown_outcome_instrument` ~1203-1234 matches `market_slug` + `Up`/`Down` outcome with NO
  `binary.id.venue` check) + `src/strategies/binary_oracle_edge_taker.rs` (~2142 feeds the
  selection the WHOLE global NT cache via `cache.instrument_ids(None)`). Venue is only
  self-consistency-checked (up_venue == down_venue, updown.rs ~1004), never tied to the
  execution client's venue. A colliding slug+identity instrument on another venue could be
  selected. FIX (fail-closed): scope selection to the execution client's configured venue —
  filter the instrument set to that venue (replace `instrument_ids(None)` with a venue-scoped
  query) AND/OR add a venue guard in `updown_outcome_instrument`/`selected_market_requirement`
  asserting the selected pair's venue equals the configured execution venue. Prefer doing BOTH
  (defense in depth). Must not change single-venue behavior.
- **P5-1** `src/bolt_v3_market_families/mod.rs` (`market_identity_plan_from_config` ~244-249) —
  hardcodes `updown::plan_market_identity` dispatch, bypassing `VALIDATION_BINDINGS` that the
  other 5 family operations use. FIX: route plan-building through a registry binding entry, same
  pattern as the siblings (single dispatch path).
- **P5-2** `src/bolt_v3_providers/polymarket.rs` (`build_market_slug_filter` Err arm ~738-743)
  — warns then returns `Vec::new()` on clock/period error. It is fail-CLOSED (universe narrows
  to zero, proven vs NT `6e059dc`), but silent. FIX: escalate `warn!`→`error!` or fail-closed at
  startup so silent data-starvation is loud. **(This file is owned by the providers cluster.)**
- **P5-3** `src/bolt_v3_market_families/mod.rs` (`select_binary_option_market_from_target_with_bindings`
  ~329-341) — returns silent `None` for an unknown family; sibling dispatchers return
  `UnsupportedFamily`. FIX: return `Result<Option<_>, InstrumentFilterError>` (or equivalent)
  and converge with the siblings — fail loud on unknown family.
- **P5-4** `src/bolt_v3_market_families/mod.rs` (`instrument_filters_from_config[_with_bindings]`
  ~251-286) + `src/bolt_v3_instrument_filters.rs` (~11-59) — dead production path (only test
  callers); production filters run through `market_identity_plan_from_config`. FIX: delete the
  unused path (NO DUAL PATHS), or make production consume it. Verify zero non-test callers first.
- **P5-6** `src/bolt_v3_market_families/updown.rs` (`target_runtime_string` ~865-874) — two
  `.expect()` on a generic `T: Serialize`. Unreachable today (callers pass validated enums) but a
  latent panic. FIX: return `Result` and propagate.
- **P5-7** dup `format_target_prefix`: `src/bolt_v3_instrument_filters.rs` (~144-154, pub(crate))
  vs `src/bolt_v3_market_families/updown.rs` (~632-642, private). FIX: delete the private copy;
  call the `pub(crate)` one (NO DUAL PATHS).
- **P5-8** `src/bolt_v3_market_families/updown.rs` (`select_market_from_instruments` ~881-883) —
  `.ok()?` swallows period/clock errors to silent `None`. FIX: converge with P5-3 policy (return
  Result / fail loud); cadence is config-validated so this is defensive.
- **P5-10** `src/strategies/binary_oracle_edge_taker.rs` (~86 `rotating_market_family: String`)
  — accepts any value at parse. Startup validation rejects unknown families loud, so
  defense-in-depth only. FIX (optional): validate against `validation_bindings()` at parse.

## DISPROVEN / scope-drift (do NOT touch)
- **P5-9** (Gemini) — mismatched activation/expiration "dangerous aggregation": DISPROVEN as a
  hazard; the 4-field identity cross-check (`candidate_market_for_slug` ~1163-1168) gates pairing
  before the conservative min/max. No change.
- **P5-11** (Gemini) — core validation traversing gate_subscriptions/market_mappings: SCOPE-DRIFT
  to P2; not a P5 concern.

## Fix-landing status (current head, 2026-06-01)

Re-verified vs HEAD (verification workflow + personal):
- **FIXED-IN-CODE (landed in `dfb4a44e`):** P5-1, P5-2, P5-4, P5-6, P5-7, P5-10.
- **DEFERRED-OK:** P5-5 (multi-venue) — the loud deferral comment + the `up_venue == down_venue`
  self-consistency guard are both present at the selection site; the cross-venue invariant is
  unreachable single-venue and tracked for the multi-venue workstream.
- **P5-3 — FIXED (this slice):** the unknown-`family_key` branch of
  `select_binary_option_market_from_target_with_bindings` now emits a loud `error!` instead of a
  silent `None` (the "or equivalent" fail-loud the finding asked for). The signature stays
  `Option` deliberately — converting the live-money strategy/operator selection chain to `Result`
  for a branch that **cannot** be reached (P5-10 rejects unknown families at config load) is the
  wrong risk/reward; the loud guard makes the should-never-happen visible without the cascade.
- **P5-8 — FIXED (this slice):** the `i64::try_from` and `updown_period_pair` `.ok()?` swallows in
  `select_market_from_instruments` now emit a loud `error!` before returning `None` (same rationale;
  cadence is config-validated positive and `now` is clock-sourced, so both are unreachable faults).

**No remaining unaddressed P5 finding.** Ready for external re-review (base `1f6ee056` → current head).

## Coverage cross-check (2026-06-01) — deferral tracking confirmed

- **Gemini F6 / P5-11 (family-neutral validation encapsulation) — deferral now tracked in P2.** This record
  classified P5-11 as SCOPE-DRIFT to P2 and took no P5 action, but the deferral target had never been shown
  to capture it — a phase-boundary tracking gap. Re-verified at HEAD: `validate_target_gate_provider_references`
  (`src/bolt_v3_validate.rs:1455`) and `collect_chainlink_target_mapping_references` (`:1613`) still traverse
  family-specific target shapes (`gate_subscriptions` / `market_mappings`) in the family-neutral validation
  engine. It is a non-safety design-cleanup item (validation-only; no real-money path). Now recorded under
  "Cross-check (2026-06-01)" in `P2-adjudication.md` so it cannot fall through the crack.
- **P5-5 (venue-unscoped selection) — remains DEFERRED-OK, not a fix.** Re-confirmed: `updown_outcome_instrument`
  (`updown.rs`) still has no `binary.id.venue` check and the strategy feeds the whole global cache; the
  deferral is correctly recorded (loud comment + operator sign-off + `up_venue == down_venue` self-consistency
  guard) and is unreachable under the single-venue config. Flagged here only so the deferral is not mistaken
  for a landed fix; venue-scoping stays with the multi-venue workstream.
