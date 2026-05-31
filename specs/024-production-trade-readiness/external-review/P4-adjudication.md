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

## Coverage cross-check (2026-06-01) — findings in the raw 6-model outputs not captured above

A re-read of all six raw P4 review outputs against this record surfaced four findings raised at
round-1 that this adjudication had not separately dispositioned. Each re-verified personally at
HEAD `19a3469d` (not promoted from any subagent verdict):

- **Grok Finding 2 (HIGH) — main-taker per-submit envelope binding — DISPROVEN (live-money escape).**
  Claim: the operator-approved envelope is hash-validated once at runner entry, but the main taker
  re-derives decision inputs (`price_to_beat`, etc.) per submit and never re-binds them, so a drifted
  input could fire an unapproved real order. Disproof, three anchors re-read at HEAD:
  1. **The main taker cannot fire during the operator-approved canary.** `build_live_node_with_clients`
     (`src/bolt_v3_live_node.rs:3242-3270`) registers **only** the canary proof executor when
     `proof_executor_enabled`, with the production strategy summary set to `registered: Vec::new()`;
     production mode registers the taker and disables the proof executor. The two are mutually
     exclusive. Proven by the existing test
     `bolt_v3_live_node_build_registers_only_generic_canary_proof_executor_when_enabled`
     (`tests/bolt_v3_strategy_registration.rs:1419`) → `registered_strategy_ids() ==
     ["canary-proof-executor-proof"]`.
  2. **Every admitted order is bounded by the operator-sealed gate report.** `admit`
     (`src/bolt_v3_submit_admission.rs:135,145`) hard-rejects `notional > max_notional_per_order()`
     and `admitted_order_count >= max_live_order_count()` from the report armed once at
     `run_bolt_v3_live_node` (`:2188`). Order **size** is config-driven (`order_notional_target`),
     not derived from `price_to_beat`; the instrument is bound to `selected_market` via the
     entry-verified `readiness_evidence` in the snapshot (`binary_oracle_edge_taker.rs:4263-4275`).
  3. **On-disk evidence tamper is inert.** The per-submit snapshot reads the **in-memory**
     `self.active.price_to_beat` (`binary_oracle_edge_taker.rs:4163`), set once from the
     entry-SHA-verified seed (`:1973`; entry SHA-check at `tiny_canary_evidence.rs:1783`); the file
     is not re-read per submit. `price_to_beat` drift can only change the entry/side **decision**, never
     order magnitude, instrument, or count. A per-submit envelope re-hash is therefore defense-in-depth
     against in-process memory tampering (outside the threat model). **DISPROVEN.**
- **Gemini #3 (HIGH) — sizing-tick freshness — FIXED (by elimination).** `market_order_cache_price_for_order`
  (`binary_oracle_edge_taker.rs:4120`) and `quote_quantity_reference_price_for_order` (`:4142`) read the
  NT cache with no `ts_event` age check, and are reached **only** when `order.is_quote_quantity()` is true
  (`:4104→4110`, `:4003→4004`). Exits/forced-exits already reject `is_quote_quantity`; R12 forbade market
  quote-qty entry; the only residual reachable path was **limit quote-qty entry**. Fix: the archetype now
  forbids **all** quote-quantity entries (`check_entry_order_combination`,
  `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs`), so `order.is_quote_quantity()` is unreachable for
  any order this archetype produces → the stale-tick sizing path cannot fire. Production config is
  unaffected (`is_quote_quantity = false` throughout). Test:
  `bolt_v3_archetype_rejects_quote_quantity_limit_entry_order` (`tests/config_parsing.rs`). Re-enabling
  quote-quantity (deferred **#506**) now carries BOTH the order-template fanout model AND a submit-time
  cache-tick freshness guard as recorded requirements.
- **Gemini #4 (HIGH) — canary quote-quantity denomination — FIXED (at load).** With
  `proof_policy.is_quote_quantity = true` the executor passes a base share count to NT's
  `order_factory().limit(.., Some(true), ..)`, which NT denominates as a quote-currency amount (wrong
  notional). It failed closed downstream via `rounded_order_admission_notional`, but is now rejected at
  load: `validate_live_canary_proof_policy` (`src/bolt_v3_validate.rs`) forbids
  `is_quote_quantity = true`. Production canary config has `is_quote_quantity = false`. Test:
  `bolt_v3_live_canary_proof_policy_rejects_quote_quantity` (`tests/config_parsing.rs`).
- **Kimi F6 / A11 — DISPROVEN anchor re-confirmed VALID.** The cross-check flagged the A11 disproof for
  citing `normalize_order_sizing` as non-existent. Re-verified at HEAD: `normalize_order_sizing` **exists**
  at `src/bolt_v3_canary_proof_policy.rs:243`, is the sole sizing path
  (`select_canary_proof_candidate`, `:239`), and enforces step-floor (`:256`), `min_quantity` (`:266`),
  and `min_notional` (`:274`) fail-closed before the order-intent is built. The original A11 DISPROVEN
  stands; the cross-check claim was the error.
