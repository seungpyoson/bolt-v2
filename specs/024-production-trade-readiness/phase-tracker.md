# PR #480 — Phase Tracker (P1–P7)

> **Purpose.** P1–P7 are the **MECE decomposition of the entire PR #480 scope** —
> **M**utually **E**xclusive (no two phases own the same concern) and
> **C**ollectively **E**xhaustive (together they cover *all* of PR #480's
> production-trade-readiness scope, T036–T047, with nothing left uncovered).
> Each phase is closed by an external adversarial review.
>
> **Why this file exists.** The phase list was previously tracked only in
> conversation and was lost across a context compaction. This file is the single
> **durable** source of truth for phase identity + status. Update it whenever a
> phase opens, closes, or its scope changes. Never let the phase list live only
> in chat again.

## Status legend
- ✅ **CLOSED** — adversarial review passed; evidence recorded.
- ⬜ **OPEN** — not yet closed.
- 🔧 **RECONSTRUCTED** — phase identity inferred from on-disk + git evidence, **not
  yet confirmed** against the original phase plan. Pending transcript recovery
  (`~/.claude/projects/*bolt-v2*/*.jsonl`). Replace with the confirmed scope once found.

---

## Confirmed phases

### P1 — Submit-cap / order-admission safety  ✅ CLOSED
- **Scope:** per-order notional cap as a hard ceiling; quote-quantity admission +
  floors on both sides; market-style entries valued at the instrument price ceiling.
- **Evidence:** commit `d61e6098` body — *"the earlier submit-cap (P1)"*; commits
  `e12574ba`, `1bf938be`, `b89eae3e`, `638a095d`, `97da6236`.
- **Code:** `src/bolt_v3_submit_admission.rs`.

### P6 — Hardcodes audit (NO HARDCODES)  ✅ CLOSED
- **Scope:** zero runtime-value literals in production Rust; runtime-literal
  allowlist + full `just source-fence` (all verifier pairs incl provider-leak).
- **Evidence:** `external-review/p6-hardcodes.md` (6/6 reviewers); project memory.
- **Related open task:** T047 (final hardcode/architecture cleanup audit) — its old
  blocker (red clippy + allowlist drift) is now cleared by CI-green HEAD; needs
  re-verify + un-stale the `tasks.md` "NOT complete" note.

### P7 — `binary_oracle_edge_taker` strategy bindings (NO HARDCODES + NO DUAL PATHS)  ✅ CLOSED
- **Scope:** every strategy binding config-sourced and singular; reference /
  decision-reference provider wiring; no dual-path.
- **Evidence:** `external-review/p7-binary-oracle-bindings.md`; commits
  *"P7 #4 disproven"* (`5633fec0`), *"P7 attack #4"*, `10653997`.

---

## Open phases — recovered from transcript (P2–P5)

Recovered from the session transcript (`~/.claude/projects/*bolt-v2*/*.jsonl`):
PR #480 was decomposed as a **domain-layer MECE**. Transcript anchors:
`P1=legacy paths, P2=TOML/config, P3=provider/secrets, P4=strategy/policy`,
`P5=market family`, and `P1 = order-admission gate, P6 = bolt's own
hardcode/config discipline, P7 = [verifiers/bindings]`.

### P2 — TOML / config  ✅ CLOSED
- Production-readiness review of the config layer (validation, schema, fixtures,
  operator TOML). Code: `src/bolt_v3_config.rs`, `src/bolt_v3_validate.rs`.
- **Evidence:** `external-review/P2-adjudication.md` (6 models; reviews at
  `1f6ee056`, every finding re-verified vs HEAD). One real fail-closed-at-load gap
  (F2 — non-positive / mis-ordered strategy sizing in `validate_parameter_bounds`)
  FIXED + 3 tests; F1 already-fixed (`f54181f0`); F3/F5 disproven as hazards (both
  fail closed before NT's runner loop); F4/F6 nits (1 doc fix, rest declined). No
  live-money hazard found.

### P3 — Provider / secrets  ⬜ OPEN
- Provider bindings + secret resolution (SSM single source, zeroize, no-leak).
  Code: `src/bolt_v3_providers/*`, `src/bolt_v3_secrets.rs`.

### P4 — Strategy / policy  ⬜ OPEN
- Strategy/policy layer beyond the P7 binding audit: decision/entry policy plus
  **no-submit-readiness and the canary** (operator-confirmed these fold here).
  Code: `src/strategies/binary_oracle_edge_taker.rs`,
  `src/bolt_v3_no_submit_readiness.rs`, `src/bolt_v3_live_canary_gate.rs`,
  `src/bolt_v3_canary_proof_*`, `src/bolt_v3_decision_evidence.rs`,
  `src/bolt_v3_submit_admission.rs`.

### P5 — Market family / instrument filter  ⬜ OPEN
- Market-family + instrument-filter layer. Code: `src/bolt_v3_market_families/*`,
  `src/bolt_v3_instrument_filters.rs`.

> **RESOLVED (operator-confirmed).** P6 = bolt's own hardcode/config discipline
> (operator line: *"P1 = order-admission gate, P6 = bolt's own hardcode/config
> discipline, P7 = …"*). The `P6 = readiness/canary` label was the **PR #331**
> P1–P9 template bleeding in — a different PR. For #480: **no-submit-readiness +
> canary fold into P4 (strategy/policy).**

> **Reference — PR #331 P1–P9 template** (do not confuse with #480): P1=legacy
> paths, P2=TOML/config, P3=provider/secrets, P4=strategy/policy, P5=market
> family, P6=readiness/canary, P7=verifiers, P8=docs, P9=supporting.

---

## Add-on (outside the original P1–P7)
- **Rate-limit / venue-egress reconciliation (Tier-1).** Align the NT submit/modify
  throttle to the Polymarket REST egress cap (`HTTP_RATE_LIMIT`); fail-loud config
  validation. Commits `d61e6098`, `146ac574` (+ Gemini cleanup `5cf96655`,
  `0f5a5704`). CI green. You noted this was "just added," i.e. not one of P1–P7.
  Tier-2 (full shared REST budget) tracked in #488.

---

## Task systems — how they relate (DO NOT CONFLATE)

Three decompositions touch this PR and all use the word **"phase"** — the #1
confusion risk:

| System | "Phase" means | Source of truth | Role |
|---|---|---|---|
| **spec-kit `tasks.md`** | implementation Phases 1–8 (by User Story) | `tasks.md` | **Authoritative work backlog** (T0xx). 107 done / 5 open. |
| **P1–P7 (this file)** | adversarial **review** domains | `phase-tracker.md` | Quality overlay on already-built code. NOT in tasks.md. |
| **recovery-plan R1–R13** | recovery remediation steps | `recovery-plan.md` | R1–R7 ~done (CI green); **R8–R13 == spec-kit Phase 8 closeout**. |

**Rule:** `tasks.md` is the authoritative backlog. P1–P7 is a review overlay
cross-linked here, not a competing list. Recovery R8–R13 and spec-kit Phase 8
are the SAME closeout chain — track it in `tasks.md`.

## Unified open work (each item listed once)

Implementation is essentially complete (107/112 spec-kit tasks). What remains =
the 5 open spec-kit tasks + closing the 4 open review phases.

| # | Work | spec-kit task | review phase | autonomous now? |
|---|---|---|---|---|
| 1 | Adjudicate rate-limit/venue-egress external responses | (add-on) | add-on | **yes** (operator pastes outputs) |
| 2 | Adversarial review — config — ✅ CLOSED (`P2-adjudication.md`; F2 fixed) | — | **P2** | done |
| 3 | Adversarial review — provider/secrets | — | **P3** | **yes** |
| 4 | Adversarial review — strategy/policy | — | **P4** | **yes** |
| 5 | Adversarial review — market-family/instrument-filter | — | **P5** | **yes** |
| 6 | Final hardcode/architecture cleanup (re-verify + un-stale note) | **T047** | ≈P6 (closed) | **yes** (CI-green cleared the blocker) |
| 7 | Data-client venue-neutral readiness matrix | **T043A** | ~P3/data | partial (evidence) |
| 8 | Freeze FINAL_HEAD; re-run no-submit on EC2 | T043/T043B | — | no (EC2/SSM/EIP) |
| 9 | Tiny-capital canary — **REAL money** | **T044** | P4 live-fire | no (EC2 + operator hard-yes) |
| 10 | Post-run hygiene | **T045** | — | no (after T044) |
| 11 | Readiness ledger + issue/PR updates | **T046** | — | partial (draft now) |

**Dependency chain (spec-kit Phase 8):** `T047 → freeze FINAL_HEAD →
T043/T043B no-submit (EC2) → T044 canary (real money, operator approval) →
T045 → T046`. T043A is parallel (gates the multi-venue claim, not the
selected-path canary).

**Autonomous-now batch = rows 1–7. Operator/EC2-gated tail = rows 8–11.**
