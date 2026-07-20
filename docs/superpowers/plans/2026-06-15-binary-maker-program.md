# Binary-Oracle Maker — Implementation Program (Slice Index)

> **For agentic workers:** This is the **program backbone**, not a task-level plan.
> The binary maker is ~11 independently-shippable slices; each slice gets its own
> bite-sized TDD plan under `docs/superpowers/plans/` and is executed via
> `superpowers:subagent-driven-development`. This document fixes the slice
> decomposition, the §16-decision → slice mapping, the dependency order, and the
> definition of done. Implement slices in the order given; do not start a slice
> until its `Depends` slices are committed on the single program branch.

**Goal:** Build the venue/instrument-agnostic market-making platform on
NautilusTrader, proving it with the Polymarket binary up/down outcome token as the
first instrument — gated on a backtest of the maker *as built* (FR-001).

**Architecture:** One shared engine on NT + two per-asset slots (pricing model,
settlement) behind the existing `MarketFamily` seam. The binary pricing/quoting/
inventory helper libs already exist on `main` (unwired, from #580); this program
wires them into a registered maker archetype, builds the genuine net-new residue
(μ estimator, composite exposure, graduated governor, settlement booking), ports
the quote-lifecycle pipeline from `codex/reference-price-architecture`, and adds
the backtest go-live gate.

**Tech Stack:** Rust, NautilusTrader Rust API (rev `6e059dc`), TOML config,
Python CI fences (`scripts/verify_bolt_v3_*.py`), `just` recipes.

**Source spec (approved):** `docs/superpowers/specs/2026-06-14-multi-asset-mm-platform-design.md`
@ `790da9a2e`. Issue: **#488**. FR spec: `specs/488-binary-oracle-maker/spec.md`.

**Pinned anchors:** bolt `main` `e2c726c2` · NT `6e059dc`
(`/Users/spson/.cargo/git/checkouts/nautilus_trader-3c6af4345b4d438b/6e059dc`) ·
pipeline source branch `codex/reference-price-architecture` @ `d4159c0a9`.

---

## Standing constraints (apply to every slice)

- **NO HARDCODES** — every runtime value from TOML. **NO DUAL PATHS** — one way per
  thing. **NO DEBTS** — no TODO/`fix later`/unpinned deps/uncommitted work.
- **NT-first strip mandate:** if NT provides it, use NT's and strip ours. Build only
  genuine residue NT lacks; each port re-verified at file:line, hardcodes→TOML,
  panics→Result.
- **Local cargo is REFUSED** — use the current local gates from `AGENTS.md`, then
  `git push` and let advisory CI evaluate the exact head. Never wait on a local full run.
- **ONE BRANCH / PR FOR THE WHOLE PROGRAM** (user directive, 2026-06-15). All slices
  ship on a single branch (`feat/488-generic-maker`) and a single draft PR (#716);
  the declared scope is the #488 binary-oracle maker program. Slices are
  dependency-ordered build units within it — each CI-green + adversarially reviewed
  (Codex + internal) as it lands — not separate PRs.
- Every `lib/`-style module that writes shared state needs unit tests before merge.

---

## Slice decomposition

Each slice ships working, testable software on its own. `§16#n` = the Must-Resolve
decision the slice closes.

### Slice 0 — Drift fences & shared feed-health seam · `§16#12 §16#9 §15`
**Ships:** the FR-080 venue-name CI fence (closes the §9 enforcement gap) + the
stale-feed gate hoisted into a shared `bolt_v3_feed_health` module (removes the
`pub(super)` barrier so the maker can reuse it) + the stale `spec.md`/`plan.md`
assumption fixes (§14#7/#8, §16#9). No maker code. **Depends:** none.
**Plan:** `2026-06-15-binary-maker-slice0-fences-and-feed-health.md`

### Slice 1 — Maker archetype skeleton + live registration + source integrity · `§16#1 §16#2`
**Ships:** a registered but inert `BinaryOracleMaker` strategy under
`src/strategies/binary_oracle_maker/` that compiles, registers via an **injectable**
`StrategyRuntimeBinding` slice hoisted to a non-scanned caller (NOT a taker-archetype
mirror), and passes a new `MAKER_KEY` `GatedSourceRoot` + `GOLDEN_MAKER_DIGEST`
(`STRATEGY_KEY` untouched). Proves the registration + source-integrity path
end-to-end before any behavior. **Depends:** Slice 0.

### Slice 2 — μ estimator + health gate · `§16#6`
**Ships:** a signed-flow aggressor-side classifier + informed-fraction (μ) estimator
fed by the config-activated `(Trade,Trade)` subscription (FR-021), plus a **μ-health
gate** that blocks quoting AND go-live when μ is absent, stale, NaN, or constant-0.
**Depends:** Slice 1.

### Slice 3 — Canonical pricing chain · `§16#8`
**Ships:** a `GmReservationBand` newtype produced **solely** by `gm_binary_quote(p, μ)`
and consumed by `compose_binary_legs`; the reservation fields become unconstructable
by a bare struct literal (type-level "sole producer"). Closes the §4 CANONICAL-CHAIN
GAP (today compose runs with no GM upstream). **Depends:** Slice 2, Slice 1.

### Slice 4 — Maker quote sizing seam · `§16#13`
**Ships:** an engine-side maker sizing input that feeds an edge/half-spread proxy
(NOT raw `choose_robust_size`, which returns ZERO at non-positive EV — the GM/CG
break-even regime); `QuoteTargetLeg`/`QuoteTargets` carry size. **Depends:** Slice 3.

### Slice 5 — Composite inventory exposure · `§16#4`
**Ships:** the single net-new composite exposure snapshot the admission gate reads —
NT Portfolio filled positions + `cache.orders_open` + `cache.orders_inflight`,
reconciled through the No-leg sign adapter (`net_yes = yes − no`); `MakerInventory`
(confirmed-fill-only) is at most one input. **Depends:** Slice 1.

### Slice 6 — Quote-lifecycle controller + requote budget · `§16#3` · `FR-010..013`
**Ships:** the six pipeline files (`bolt_v3_maker_{quote_plan,quote_control,quote_set,
order_plan,order_compile,order_dispatch}.rs`) + the `MarketAction`→NT executor ported
from `codex/reference-price-architecture` @ `d4159c0a9` into the generic archetype,
with cancel+resubmit reprice (no venue modify) and a **two-budget** requote model —
submit-governor ≤ 40/min AND venue CLOB REST ≤ 100/min, reserving the cancel+resubmit
pair atomically as one acquisition. **Depends:** Slice 3, Slice 4, Slice 5.

### Slice 7 — Risk governor + loss backstop + active-flatten decision · `§16#7 §16#11` · `FR-023`
**Ships:** the graduated governor (cancel-only / reduce-only / hard-flat / soft-hold)
with TOML fail-closed thresholds; loss-governor **firing code** computing running P&L
against a parsed config field (replacing the orphaned deploy-local `loss_governor`);
and the resolved active-flatten decision (wire a taker-mode crossing reduce off
`inventory_skew`'s `None`, OR scope hard-flat OUT for the binary slot with documented
residual settlement-loss risk). **Depends:** Slice 5, Slice 6.

### Slice 8 — Settlement slot · `§16#5 (a/b/c)`
**Ships:** the per-asset settlement slot — resolution-signal source (decision **5a**,
see "Strategic Decision" below), winner detection from the `"Winner: …"` reason string
vs the instrument `token_id`, explicit 0/1 payout booking + inventory reset, the
double-booking authority (5b), and the `maker_settlement_payout` signature change to
accept the resolved payout (5c). One shared settlement primitive across backtest+live
(FR-004). **Depends:** Slice 5, Slice 1. **Carries the one strategic decision.**

### Slice 9 — Market selection "B" · `§7` · Task #1
**Ships:** the shared selection mechanism — auto-discover candidate markets +
eligibility filter + concurrency cap + auto-rotate, with the portfolio layer that
splits capital and isolates per-market state/health/kill (FR-041). Edge-ranking "C"
stays deferred (needs a backtest profitability estimate). **Depends:** Slice 1 (can
run parallel to Slices 2–8).

### Slice 10 — Backtest go-live gate · `§16#10` · `FR-001..005` — **DEFINITION OF DONE**
**Ships:** the FR-001 backtest of the maker *as built*, scored
`net = captured-spread − fees − adverse-selection − settlement-loss` over a real
historical full-depth L2 window, against thresholds registered **before** the run,
PASS/FAIL — run with `queue_position=true` (NT default is `false` → optimistic
at-touch fills) AND a corpus containing **TradeTick + OrderBookDelta** events (both
asserted in the harness). Uses NT's `ExecutionModel`; a custom FillSim only if NT's
queue model is first source-proven insufficient. **Depends:** all prior slices.
**Cross-dependency:** the TradeTick+OrderBookDelta corpus depends on the BTE epic
(#439/#696); the non-book replay gap means OrderBookDelta replay may need BTE work
first — flagged, not owned here.

---

## §16 Must-Resolve → slice coverage (all 13)

| §16 | Decision | Slice |
|----|----------|-------|
| 1 | Live-registration injectable binding slice | Slice 1 |
| 2 | `MAKER_KEY` GOLDEN-digest (not `STRATEGY_KEY`) | Slice 1 |
| 3 | Requote two-budget + atomic reservation | Slice 6 |
| 4 | Inventory composite-exposure (net-new) | Slice 5 |
| 5 | Settlement contract (signal/authority/signature) | Slice 8 |
| 6 | μ source + health gate | Slice 2 |
| 7 | Loss-backstop firing code + parsed config | Slice 7 |
| 8 | Canonical pricing chain (`GmReservationBand`) | Slice 3 |
| 9 | Correct `spec.md`/`plan.md` stale assumptions | Slice 0 |
| 10 | Backtest queue-realism (corpus + config) | Slice 10 |
| 11 | Active-flatten decision (FR-023) | Slice 7 |
| 12 | FR-080 venue-name fence | Slice 0 |
| 13 | Maker quote sizing seam | Slice 4 |

## Dependency order

```
Slice 0
  └─► Slice 1 ─┬─► Slice 2 ─► Slice 3 ─► Slice 4 ─┐
               │                                   ├─► Slice 6 ─► Slice 7
               ├─► Slice 5 ───────────────────────┘            │
               │        └─────────────────► Slice 8 ◄──────────┘ (needs S5; S6 for actions)
               └─► Slice 9 (parallel)
                                                   Slice 10 (grades all) ◄── DONE
```

Critical path: **0 → 1 → 2 → 3 → 4 → 6 → 7 → 8 → 10**. Slice 5 feeds 6/7/8; Slice 9
is parallel after 1.

## Settlement-signal decision (§16#5a) — DECIDED: Chainlink-oracle-derived outcome

The live `InstrumentStatus::Close` signal (Polymarket `MarketResolved`) is delivered
**only** to an all-markets subscriber (`subscribe_new_markets=true`), which bolt
forces `false` and fail-closes (`polymarket.rs:419-427,755-761`) because NT's pinned
client issues an all-markets `subscribe_market(vec![])` on that flag
(NT `data.rs:1189-1191`) — violating the controlled-connect boundary. Three options
were grounded against source:

| Opt | Approach | Respects boundary | Feasibility | Verdict |
|-----|----------|-------------------|-------------|---------|
| A | Lift `subscribe_new_markets=true` | ✗ (needs pinned-NT-adapter surgery; per-token channel likely never carries `MarketResolved`, `messages.rs:180-181`) | PLAUSIBLE-UNVERIFIED | **Rejected** |
| **C** | **Derive outcome from the Chainlink strike oracle** at window-close (`close ≥ strike → Up/Down`) | ✓ (zero WS/fence change) | CONFIRMED-EXISTS | **CHOSEN** |
| B | Poll Gamma REST `GET /markets/{id}` → `closed` + `outcome_prices` | ✓ | CONFIRMED-EXISTS | **Reconciliation cross-check only** |

**Decision — Option C.** The binary up/down outcome is definitionally "underlying
close vs strike," and the strike already comes from a point-in-time Chainlink Data
Streams client (`src/bolt_v3_providers/chainlink/strike_source.rs:61,268-300,375-443`)
bound per asset (`root.toml:725-765`); the 0/1 settlement primitive already exists
(`updown.rs:115-120 maker_settlement_payout`). Using Polymarket's own resolution
(A/B) would introduce a **second** resolution authority — a Rule #2/#6 dual-path. C
reuses the single oracle the system already trusts and is consistent with **CLAUDE.md
#13** (Chainlink testnet IS the production resolution anchor for this strike path).

**Net-new for Slice 8 (all small, fail-closed):** (1) a window-**close** fetch trigger
mirroring the existing window-open subscribe; (2) a close-instant report-binding rule
analogous to the F2 open-binding check (`strike_source.rs:458-476`); (3) a
`close ≥ strike → OutcomeSide` decoder (define tie-at-strike); (4) **Option B as a
reconciliation cross-check** — poll Gamma `closed`/`outcome_prices` and alert on any
disagreement with the oracle-derived outcome (bounds the rare oracle-vs-UMA basis
divergence; `gamma.rs:147`, `models.rs:174-175,201-202`). **Open verification for
Slice 8:** the close-instant binding rule + the basis-divergence bound (B is the
check). This does not block Slices 0–7.

## Execution model

- **One branch + one draft PR for the whole program** (`feat/488-generic-maker`,
  PR #716) — user directive, 2026-06-15. Declared scope = the #488 binary-oracle maker.
- Each slice plan is fully bite-sized TDD (failing test → run-fail → minimal impl →
  run-pass → commit), executed via `superpowers:subagent-driven-development`, and
  committed sequentially on the single branch.
- Each slice gets a Codex adversarial review **and** an internal adversarial review as
  it lands; every finding is FIXED or DISPROVEN before the next slice. Quality is
  checked continuously — only the merge is deferred.
- Fast local gates (fmt/clippy/`just source-fence`/targeted tests) before push; CI
  runs the full suite remotely on every push to the branch.
- **No intermediate merges to `main`.** The program merges once, under the user-gated
  `MERGE-TO-MAIN`, after Slice 10's backtest go-live gate passes. Latest `main` is
  integrated into the branch (with source-integrity seal re-recording) as a discrete
  verified step before that merge. Parallel slices (5, 9) are committed concurrently
  on the same branch.
