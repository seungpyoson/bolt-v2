# P4 Adjudication — Strategy / Policy + No-Submit-Readiness + Canary (PR #480)

HEAD `1f6ee056`. 6 external models; every finding re-verified vs HEAD bytes.
Verdict: **HARDENING-ONLY** — crown-jewel invariants HOLD: per-order notional cap is a
strict-`>` hard ceiling (submit_admission ~138), admission is fee-inclusive on every reaching
path, rounding fails closed (~414), count-cap is race-free (single lock spans evaluate+increment),
and no real order fires without the armed gate report. No confirmed live-money escape at HEAD.

Anchors use function name + file (line numbers approximate; re-locate by name + re-verify).
**Every fix must preserve or TIGHTEN fail-closed behavior — never loosen a guard or create a
new submit path. If a fix would require loosening anything, STOP and report instead.**

## CONFIRMED — actionable

- **A1 / A2 (LIVE-FIRE GATE — most delicate)** `src/bolt_v3_tiny_canary_evidence.rs`
  (`validate_approval_not_consumed` ~1931 `try_exists`; consume `~2028` `create_new`; nonce
  compare ~1978; consumption path `is_absolute` allowed ~1617). One-time approval rests on a
  deletable filesystem marker: deleting the consumption file inside the operator window with an
  unchanged nonce re-arms and re-fires with no new operator action; the consumption path may be
  an absolute /tmp path subject to host cleanup. FIX (conservative, fail-closed): bind
  consumption to the nonce (rotate/derive the marker from the nonce so deletion cannot re-arm),
  AND constrain the consumption path to a durable workspace dir (reject absolute / world-writable
  tmp). Must NOT weaken arming; only make re-arm harder. Propose the design in the report, then
  implement; do not change the gate's accept criteria.
- **A3** `src/bolt_v3_live_canary_gate.rs` (`validate_operator_canary_proof_order_intent` ~1276)
  — the candidate-source list / selection provenance fields (`canary_proof_candidate_source_*`)
  are never read at the gate; selection runs offline only. FIX (design-heavier): re-derive or
  verify the selected market at the gate against the candidate-source, or consume the currently-
  dead provenance fields. If it can't be done safely/small, report a precise plan and DEFER.
- **A4** `src/strategies/binary_oracle_edge_taker.rs` (ceiling branch ~4014, gated on `Entry`
  only) + `src/bolt_v3_order_intent.rs` (Market arm builds price-less order). A market-style
  RiskReducingExit is price-less but, being `Exit`, skips the structural price-ceiling valuation.
  FIX: extend the ceiling valuation to market-style exits with a side-appropriate worst-case
  bound, so the notional cap stays a structural bound for that shape too.
- **A5** `src/bolt_v3_live_canary_gate.rs` (gate report omits freshness ~151-158) +
  `binary_oracle_edge_taker.rs` (stale check uses `config.forced_flat_stale_reference_ms` ~2575).
  Gate validates `reference_quote_max_age_seconds` at startup but it's never plumbed into the
  submit path — two freshness policies. FIX (single-source): plumb the gate-approved freshness
  bound into the armed admission/stale check so there is ONE source of truth (GROUP-BY-CHANGE).
- **A6** `src/bolt_v3_submit_admission.rs` (inverse valued via `calculate_notional_value(..,Some(true))`
  ~322, floor skipped ~252) + `binary_oracle_edge_taker.rs` (fail-closed assert only in
  None-fallback ~3986). Inverse quote-quantity isn't fail-closed on the success path. Reachable
  only if an inverse instrument enters the universe (P3/P5 gate it), but the defense lives here.
  FIX: reject inverse quote-quantity at the shared admission (fail closed), or carry currency-aware
  settlement notional. Prefer the fail-closed reject.
- **A7** `src/bolt_v3_canary_proof_executor.rs` (`SubmitTimeTopOfBook` no `ts_event` ~459;
  cache fallback unchecked ~227). Submit-time book isn't timestamp-fresh (limit price still
  bounds the debit, so liveness-only). FIX: bind the book evidence to an event timestamp + a
  max-age and reject if stale.
- **A12** `binary_oracle_edge_taker.rs` (~4002) — exit fee uses the entry-fee method
  (`max_entry_fee_bps`); overstates → fails closed; symmetric fees today. FIX: rename or
  document the symmetric-fee assumption (low).
- **A13** `binary_oracle_edge_taker.rs` (comment ~4015-4022) — "exit-exempt" comment is
  misleading; exits DO hit the notional cap (submit_admission ~138). FIX: correct the comment.
- **A14** `binary_oracle_edge_taker.rs` (`is_some_and` ~7113; entry snapshot fails on None ~4126)
  — stale predicate returns false for an initial `None` reference. Benign (entry already fails
  on None) but classify missing-ts as stale for defense-in-depth. FIX: treat missing-ts as stale.
- **A15** `binary_oracle_edge_taker.rs` (~3921 `record_order_intent` before admission-request
  build ~3922 that can fail) — orphan evidence line if the build fails (order never fires).
  FIX: reorder so evidence is recorded after the fallible build, or document partial-chain
  semantics.

- **A-EDGE (surfaced in P2 re-review by DeepSeek, 2026-06-01)** `binary_oracle_edge_taker.rs`
  (`parameters.edge_threshold_basis_points: i64`, never range-validated at load). A
  NEGATIVE threshold makes the entry edge check (`expected_edge > threshold * theta`)
  true for any/negative edge → the strategy enters guaranteed-loss trades. No live-fire
  hazard without operator misconfig, but a nonsensical config must fail closed at load
  (same class as F2 sizing bounds). FIX: reject `edge_threshold_basis_points < 0` (or
  `<= 0` if a zero threshold is also nonsensical) in archetype parameter validation,
  with a test. Verify the shipped fixture/prod value is positive before tightening.

## DISPROVEN (do NOT touch)
A8 (fee>intent — fee-inclusive notional hard-capped by `max_notional_per_order` strict-`>`;
`order_intent.notional` is pre-fee size, not the ceiling), A10 (negative fee — `max_fee_bps>=0`
enforced both sides), A11 (quantity constraints — `normalize_order_sizing` enforces step/min),
A16 (mutex `.expect` — intentional FAIL-LOUD), A17 (count-cap race — single lock spans
evaluate+increment). A9 (blocks on zero admitted orders) is correct fail-closed for a bounded
canary; leave unless an open-ended mode is intended (out of scope).

## Fix-landing status (current head, 2026-06-01)

Re-verified vs HEAD (verification workflow + personal reads of the live-fire gate):
- **FIXED-IN-CODE (landed in `dfb4a44e`):** A4, A5, A6, A7, A12, A13, A14, A15.
- **A-EDGE — FIXED (this slice):** negative `parameters.edge_threshold_basis_points`
  rejected at load (`validate_parameter_bounds`); test in `tests/config_parsing.rs`.
- **A1/A2 — FIXED (this slice):** the one-time operator approval is now durably one-time.
  On consume, after the consumption marker is written, the process SPENDS the nonce —
  `spend_phase8_approval_nonce` overwrites the operator nonce evidence file so it no longer
  hashes to the approved `approval_nonce_sha256`. A deleted **or** host-auto-cleaned marker
  therefore cannot re-arm: `validate_approval_nonce` fails on the spent nonce. This is the
  single point where bolt-v3 writes operator evidence — by design, a nonce is single-use;
  re-approval requires a fresh nonce. (A durable-path guard for the marker was considered
  and dropped: it is redundant — the nonce-spend already defeats the temp-dir auto-cleanup
  vector — and it conflicted with the temp-dir test harness.)
- **A3 — DEFERRED (recorded plan; operator decision 2026-06-01):** the gate hash-binds the
  order-intent file to the sealed envelope and checks the order's instrument is in
  `gate_session.selected_market`, but never re-derives the selection from
  `canary_proof_candidate_source_*` (those provenance fields stay dead). Residual gap: a
  *different* market that is also in the session could pass. PLAN (dedicated PR): at the
  gate (1) bind + sha-verify the candidate-source artifact to the envelope (as the
  order-intent file already is), (2) parse the candidate pool, (3) re-run the deterministic
  market selection against the gate clock, (4) reject if the fired instrument != the
  re-derived winner. DEFERRED because it is a design-heavier change in the live-fire path
  (new bound artifact, selection re-derivation, new failure modes + tests), best done
  deliberately with its own review rather than rushed before T044. Tracked with #503.

**Remaining open:** A3 (deferred with the plan above). A1/A2, A-EDGE, and the 8 `dfb4a44e`
items are addressed; P4 is review-ready once A3's deferral is accepted by the re-review.
