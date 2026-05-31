# P2 Adjudication — TOML / Config Layer (PR #480)

Reviews ran at HEAD `1f6ee056` (base `0f5a5704`), 6 external models (DeepSeek,
GLM, GPT, Gemini, Grok, Kimi) — fix-set reviews of an already-hardened P2. Every
finding was re-verified against **current HEAD** (the commit carrying this record),
*not* the old review head — several were already fixed downstream. Each
safety-relevant verdict was re-reproduced personally (file:line re-read + test
run), never promoted from the model output.

Verdict: **P2-CONFIG-SOUND (after fix).** One genuine fail-closed-at-load gap
(F2, strategy sizing bounds) is FIXED in this change. **No live-money hazard
found:** F2 is fail-SOFT (runtime triple-guarded — a bad order can never fire),
and the two claimed hazards (F3 readiness fail-open, F5 operational-field load
gap) are DISPROVEN — both fail closed before NT's runner loop.

**Closure status: NOT CLOSED.** This is the *author's* adjudication, not phase
closure. P2 closes only on an external 6-model adversarial re-review PASS of the
F2 fix + this record — the same external gate that closed P6/P7. Self-adjudication
is not closure.

Anchors use function name + file (line numbers approximate; re-locate by name).
**Every change preserves or TIGHTENS fail-closed — no guard loosened.**

## FIXED

- **F2 (GPT; reviewer "LIVE-MONEY-CRITICAL" → downgraded to fail-soft) — FIXED.**
  `validate_parameter_bounds` (`src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`)
  accepted non-positive `parameters.order_notional_target` and
  `parameters.maximum_position_notional`, and an `order_notional_target` exceeding
  `maximum_position_notional`; the only prior rejecting arm was
  `order_target > default_max`. Runtime is triple-guarded (`choose_robust_size`
  returns 0 on cap ≤ 0; `sized_notional` positive-finite filter; `quantity_value`
  positive-finite check → `try_submit_entry_order` submits nothing), so the hazard
  is fail-SOFT (strategy silently dead with zero fills), **not** live-money — a bad
  order cannot fire. But a nonsensical config must fail loud at load. FIX
  (class-level, fail-closed): reject `order_notional_target ≤ 0`, reject
  `maximum_position_notional ≤ 0`, reject `order_notional_target >
  maximum_position_notional`. Tests (`tests/config_parsing.rs`):
  `bolt_v3_archetype_rejects_non_positive_order_notional_target`,
  `bolt_v3_archetype_rejects_non_positive_maximum_position_notional`,
  `bolt_v3_archetype_rejects_order_notional_target_above_maximum_position_notional`.
  RED confirmed (validator returned `[]`), GREEN after fix; all 187 config_parsing
  tests pass incl. `shipped_binary_oracle_config_*` (real config still loads:
  `5.00 ≤ 10.00`, both positive).

- **F6(c) (NIT, doc accuracy) — FIXED.** Schema doc `default_max_notional_per_order`
  said "decimal string"; code enforces positive (`validate_risk_block`,
  `src/bolt_v3_validate.rs`). Changed to "positive decimal string" (matches the
  sibling `[risk.nautilus].max_notional_per_order` wording). Schema verifier
  (`scripts/verify_bolt_v3_schema_current.py`) re-run: OK.

## ALREADY-FIXED (downstream of the review head)

- **F1 (required change; GLM / GPT / Gemini) — RESOLVED-IN-CODE (`f54181f0`).** The
  unmodeled-egress test asserted stale ``venue=`BINANCE` ``; the validator emits
  `(provider=BINANCE)`. Commit `f54181f0` realigned the assertion to provider
  vocabulary. Test `fails_closed_on_execution_client_for_unmodeled_egress_venue`
  re-run at HEAD: passes (genuinely exercises the fail-closed path again).

## DISPROVEN (as hazard)

- **F3 (Kimi, "fail-OPEN") — DISPROVEN.** `min_observed_targets = 0` is rejected
  fail-closed for ALL probe modes by `validate_readiness_probe_min_observed_targets`
  (`src/bolt_v3_live_node.rs` ~1380), called unconditionally before any mode branch
  (~1415) and before NT's runner. Reproduction test
  `data_client_readiness_probe_rejects_zero_min_observed_targets` (~4172, the
  finding's exact Book + MetadataResponse + `Some(0)` scenario) re-run personally:
  passes. Upper-bound (`min_observed ≤ sampled`) checks exist for every mode. The
  config-schema validator only checks `> 0` in the trade-chunk branch, but that is
  non-load-bearing (runtime guard catches it before any order). Defense-in-depth
  config-layer mirror declined: both paths already fail closed; mirroring risks a
  dual source of truth.

- **F5 (DeepSeek, HARDENING) — DISPROVEN as hazard.** The 6 operational
  `[live_canary]` fields (`no_submit_readiness_report_path`,
  `max_no_submit_readiness_report_bytes`, `readiness_report_max_age_seconds`,
  `reference_quote_max_age_seconds`, `reference_quote_wait_timeout_seconds`,
  `reference_quote_probe_actor_id`) are validated only at the runtime gate, not at
  config load — but the gate (`check_bolt_v3_live_canary_gate...`,
  `src/bolt_v3_live_canary_gate.rs` ~647) fails closed on each (empty / zero /
  invalid → early `Err`) and runs before NT's runner loop with `?`-propagation
  (`run_bolt_v3_live_node`, `src/bolt_v3_live_node.rs` ~2181 — gate, then
  `node.run()`). No bad value reaches a live-submitting runner. Residual =
  fail-loud-attribution timing only. Load-time pre-filter declined: the in-code
  comment (`validate.rs` ~179) deliberately keeps the gate as single source of truth.

## NIT — declined (recorded, no change)

- **F4 (Grok).** Over-cap egress error prints bare `{venue}` not `provider=`.
  `{venue}` is `client.venue.as_str()`; the only modeled venue is `polymarket::KEY`
  ("POLYMARKET"), so it already renders the provider token; the enforced invariant
  `!contains("(venue=")` (`tests/config_parsing.rs`) is satisfied. Cosmetic on a
  fail-closed path; declined.

- **F6(a).** Egress label uses `client.venue` not the module `KEY` const —
  divergence is structurally impossible: `validate_client_block`
  (`src/bolt_v3_providers/mod.rs` ~657) rejects any `client.venue` not equal to a
  registered binding key, so in the modeled arm `venue == KEY`. Declined.

- **F6(b).** Schema doc lacks a bulleted field table for the 6 optional
  `operator_evidence` proof-policy fields (`gate_session_path`,
  `expected_gate_session_sha256`, `canary_proof_candidate_source_path` / `_sha256`,
  `canary_proof_order_intent_path` / `_sha256`). They ARE documented in prose
  (schema ~807-809) with optional / proof-policy-mandatory / fail-closed semantics.
  A hand-maintained field table that no verifier checks would restate struct fields
  and drift — declined per "docs reference, don't restate"; the Rust struct
  (`deny_unknown_fields`) + the gate is the single owner.

- **F6(d).** Explicit `(Ok(_), Err(_))` match arm in `validate_live_canary_block` —
  the delegation to `validate_risk_block` (which runs unconditionally) is already
  documented in the in-code comment. Declined.

## Method

6 reviews consolidated → deduped into F1–F6 → one verification agent per finding
(re-read at current HEAD) → main session re-reproduced F2 (RED → GREEN) and F3
(reproduction test) personally before any state change. This record is the missing
P2 close-out: P3 / P4 / P5 / P6 / P7 / rate-limit had adjudication files, P2 did not.

## External re-review (round 1) — HEAD 83c976a5 (F2 fix fcd1d33f on base 7c063d4c)

6 models re-reviewed the F2 fix + this adjudication. **4 CLOSE-CONFIRMED** (DeepSeek,
GLM, Gemini, Kimi); **2 STILL-OPEN** (GPT, Grok) — both HARDENING/doc, no live-money.
All 6 confirmed: F2 fix correct & complete, fail-soft (not live-money), F1/F3/F5
disproofs hold, no live-money hazard survives validation.

Dissent disposition (head advances past fcd1d33f for round 2):
- **GPT #1 (schema sizing-doc drift) — FIXED.** `order_notional_target` /
  `maximum_position_notional` schema entries + the validation-rule summary now state
  positive-decimal and the `order_target <= maximum_position_notional` ordering rule
  (a loose end of the F2 fix — only `default_max`'s entry had been updated via F6c).
- **Grok #2 (position_max ≫ default_max "nonsensical") — DISPROVEN** (Kimi): a position
  cap above the per-order cap is a valid config (multiple orders fill the position).
- **Grok #3 (chunk-count `min_observed=0` bypass) — DISPROVEN** (re-verified
  personally): `required_observation_count` clamps `Some(_)` via `.clamp(1, ..)`, so a
  zero-observation pass is structurally impossible on any path; the TOML path is
  additionally rejected at `validate_readiness_probe_min_observed_targets`.
- **Grok #1 (over-cap error prints `{venue}` not `provider=`) — DECLINED** (cosmetic;
  renders the agnostic token "POLYMARKET"; the `(venue=` invariant holds; forcing
  `provider=` into adjectival prose is awkward with no behavioral gain; 5/6 accept).
- **GPT #2 / F6b (operator_evidence field table) — decline upheld 4-to-1** (Gemini /
  GLM / Kimi endorsed "docs reference, don't restate"; the struct + gate is the owner).
- **Grok #4 (positivity-error vs syntax-error wording) — NIT, declined.**

New finding surfaced (out of P2 scope → tracked in P4 as **A-EDGE**): DeepSeek flagged
`parameters.edge_threshold_basis_points` (i64) is never range-validated; a negative
value makes the strategy enter at negative edge (guaranteed-loss trades).

**Closure:** still NOT closed — the round-1 dissents (GPT/Grok) need a round-2
re-confirm of the schema-doc fix + the disproofs before 6/6.
