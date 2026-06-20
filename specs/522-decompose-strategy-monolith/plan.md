# Implementation Plan: Decompose the bolt-v3 monoliths

**Branch**: `refactor/522-decompose-strategy-monolith` | **Spec**: `./spec.md` | **Tracking**: #522

## Summary

Behavior-preserving decomposition of `binary_oracle_edge_taker.rs` and
`bolt_v3_operator_artifacts.rs` into focused modules and intended shared helpers,
slice by slice, each behind the per-slice gate in `spec.md`. The
strategy file converges to a directory module: `strategies/binary_oracle_edge_taker/`
with `mod.rs` (struct + `DataActor` orchestration + intent/signal glue) plus
submodules; genuinely shared, agnostic math/state moves to `src/` shared modules the
#488 maker reuses.

**A6 branch refresh (2026-06-06):** source identifiers below are symbol clusters, not
line numbers. Line counts are optional size telemetry only and are not source anchors.
Current `origin/main` already has A3, A4, A5, and A8 merged. GitHub reports
#507, #508, #510, and #520 are still open/unmerged, so no dependent-PR matrix entries
are removed as merged on this branch.

## Technical Context

- **Language**: Rust, workspace toolchain. NT crates at the rev pinned in `Cargo.toml`
  (single source of truth; not restated here).
- **Testing**: `cargo test` / nextest, `cargo fmt --check`, `cargo clippy`,
  `git diff --check`, `just source-fence`. CI runs the full suite (13 blocking lanes).
- **Constraints**: NO HARDCODES · NO DUAL PATHS · PURE RUST · behavior-preserving moves.

## Target Module Layout (end state, not one PR)

```text
src/strategies/binary_oracle_edge_taker/
  mod.rs            # BinaryOracleEdgeTaker struct + DataActor impl + orchestration glue + re-exports
  selection.rs      # candidate/selection snapshot construction (pure)
  exposure.rs       # ExposureState + position/recovery state machine (state-struct)
  orders.rs         # order construction + price/qty conversion (intent → NtOrderTemplate)
  source_proof.rs   # entry-decision source/replay/evidence derivation (pure)
  config.rs         # TOML config structs + parse/validate (pure)   [or fold into archetype]
  tests/            # the 238 tests, split to mirror submodules

src/bolt_v3_sizing.rs          # SHARED: pure dollar-intent sizing math (maker-reusable)
src/bolt_v3_taker_updown_signal.rs # TAKER/UPDOWN: pure EV/side-selection/uncertainty math
src/bolt_v3_taker_pricing.rs  # SHARED: reference-quote/RV/lead-venue pricing state (maker-reusable)
src/bolt_v3_book_sizing.rs    # SHARED: OutcomeBookState + VWAP/slippage execution sizing (rule #9)
# OutcomeSide (binary up/down side) consolidates INTO src/bolt_v3_market_families/ (merges with UpdownOutcomeSide; A2) — NOT taker_signal
# admission-request construction folds INTO existing src/bolt_v3_submit_admission.rs (rule #9)

src/bolt_v3_operator_artifacts/   # Track B: mod.rs (core-glue: json-io, error enum, constants, re-exports) + submodules
```

Naming is a reviewable proposal; the agnostic shared names follow the spec-023 contract
(typed-in/typed-out, no strategy/venue imports at the boundary).

## Slice Sequencing

Each row is one gated PR, ordered **by internal dependency only** (execution order =
A1→A10; A2 is foundational because A4/A6/A9 consume the side type it homes). Per
operator direction **#522 LEADS**: the open dependent PRs (#507/#510/#520/#508) are NOT
prerequisites here — they rebase onto each merged slice (see the Rebase Matrix). The
last column names which dependent PR must rebase after a slice lands. Symbol ownership,
module boundaries, focused re-exports, digest updates, and tests are the canonical
movement evidence; line ranges are not.

### Track A — strategy monolith

| Slice | Scope | Source identifier | Class | Rebases onto it |
|---|---|---|---|---|
| **A1** | **OutcomeSide-free** pure math → new `bolt_v3_taker_signal.rs`; generic numeric primitives → existing `bolt_v3_numeric.rs` | symbol cluster merged by PR #524; no line anchors | pure-logic | — (first slice; no overlap) |
| **A2** | Consolidate `OutcomeSide` into the market-family layer (merge with `UpdownOutcomeSide`; **partially resolves findings-doc #13 — OutcomeSide sub-item**); move the side-using math (`compute_worst_case_ev_bps`+`WorstCaseEvInputs`, `choose_entry_side`+`SideSelectionInputs`, `outcome_side_evidence_label`) into `bolt_v3_taker_signal` depending on that owner | symbol cluster merged by PR #526; reference repoints tracked by diff/tests, not line anchors | cross-cutting type move | — |
| **A3** | Market selection + candidate snapshot construction (pure) → `selection.rs` (**completes findings-doc #13 — the strategy-local `CandidateMarket` wrapper over market-family output**) | symbol cluster in merged `src/strategies/binary_oracle_edge_taker/selection.rs`; merged to main | pure-logic | — |
| **A4** | Order-book state + VWAP/slippage sizing → `bolt_v3_book_sizing.rs` (rule #9) | symbol cluster in merged `src/bolt_v3_book_sizing.rs`; merged to main | state-struct + pure | — |
| **A5** | Pricing state (reference/RV/lead-venue) → `bolt_v3_taker_pricing.rs` | symbol cluster in merged `src/bolt_v3_taker_pricing.rs`; merged to main | NT-actor-coupled state | #520 (SignedTradeFlow), #508 (pricing guards) |
| **A6** | Exposure/recovery state machine → `exposure.rs` | symbol cluster in `slices/A6.md`: exposure state structs/enums, support predicates, forced-flat predicates | state-struct | #507 (sizer evidence on position state) |
| **A7** | Source-proof / replay / evidence derivation → `source_proof.rs` | symbol cluster in `slices/A7.md`; merged by PR #586; no line anchors | pure-logic | — |
| **A8** | Config structs + parse/validate → `config.rs` (or archetype) | symbol cluster in merged `config.rs`; merged to main | pure-logic | #508 (config guards) |
| **A9** | Admission-request construction + valuation → `bolt_v3_submit_admission.rs` (rule #9; kill test-only dup). **Owns the base — #507/#510 rebase their admission edits onto it.** | symbol cluster in `slices/A9.md`; no stale line anchors | pure-logic | #507, #510 |
| **A10** | Split the 238 tests to mirror submodules; `mod.rs` = struct + `DataActor` + glue | `tests/{book_sizing,config,core_glue,exposure,orders_admission,pricing,selection,source_evidence,trade_flow}.rs` plus `shared_fixture.rs`; no line anchors | tests | — |

### Track B — operator_artifacts (parallel, conflict-free with Track A)

One directory module; ~13 extractable concern-modules (gate-evidence, ssm-manifest/
redaction, data-client-readiness, financial-envelope/approval-nonce,
market-selection-source, abort-plan-proof, strategy-input-evidence, chainlink-streams,
entry-decision-source, live-canary-terminal/secret-scan); core-glue (json-io, the
70-variant error enum, the 200+ constants) stays in `mod.rs`. Public API re-exported so
`tests/bolt_v3_operator_artifacts.rs` (dependent #507/#510) stays green. Sliced into a
handful of gated PRs by concern.

### Wave-2 shared-layer cleanups (after A8 lands)

canary-proof claim decoupling from shared admission (#502); `polymarket_*`→`market_*`
evidence rename (finding #12); provider credential/HTTP dedup (#447) + CLOB-v2
vendor-type relocation + fee-provider coupling (#446); live-node probe-orchestration
extraction. Tracked here, planned when their prerequisite slices land.

## Rebase Matrix (#522 leads; open dependent PRs rebase onto merged slices)

Resolves the ordering ambiguity: the dependent PRs are **not** prerequisites for any
#522 slice. Each rebases its edits onto the relevant slice **after** that slice merges.
The admission slice (A9) deliberately owns the base for the rule-#9 region so the
heavy admission editors rebase onto the cleaned module — not the reverse.

Verified via GitHub during A6 on 2026-06-06: #507, #508, #510, and #520 remain open and
unmerged. Do not delete these rows as merged evidence until their PR state changes on
GitHub.

| Dependent PR | Region it edits | Rebases after | What it rebases |
|---|---|---|---|
| #520 hoist SignedTradeFlow | strategy pricing/trade-flow symbols | A5 | the hoist re-targets the extracted pricing/trade-flow module |
| #508 causality/config hardening | strategy pricing/config guard symbols | A5, A8 | guards re-applied on the extracted pricing + config modules |
| #507 position-sizer | submit-admission, decision-evidence, and strategy exposure/admission symbols | A9 (and A6) | admission/evidence integration re-applied on the extracted admission-request module |
| #510 loss-governor | `submit_admission` +134, `live_node`, `decision_evidence` | A9 | admission rejection path re-applied on the extracted module |

If product priorities require a dependent PR to merge *before* its dependent slice,
that slice instead rebases onto the PR — but the default, per operator direction, is
#522-leads. Either way there is exactly one deterministic base at any time.

## Per-Slice Method (RED → GREEN → move → verify)

1. **Characterize**: read the exact unit at current HEAD; confirm `&self`-freedom for
   "pure" claims; enumerate inbound callers and the tests that cover it.
2. **RED**: add/relocate a characterization test pinning current behavior; show it
   discriminates (fails against a wrong control) — recorded in the ledger.
3. **Move**: relocate the unit + its tests to the target module. Re-export by caller
   scope (FR-003): for units with callers **outside** the origin, add a `pub use`
   re-export from the origin so external call sites/tests are unchanged; for units
   **private** to the origin (e.g. A1), add only in-file `use` imports and add **no**
   `pub use`. No signature/logic change.
4. **GREEN**: `cargo fmt`/`clippy`/targeted tests local; push; CI runs full suite.
5. **Verify**: moved symbol ownership, module boundary, and focused re-exports match
   the slice; the diff is a pure relocation — move + imports, with a `pub use` only
   where an external caller requires it (`git diff` shows no logic delta); for
   private-internal slices confirm **no origin `pub use` was added**. Size telemetry is
   optional context only, never an acceptance anchor.

## Risks

- **Inherent-impl spread**: splitting `impl BinaryOracleEdgeTaker` across files is legal
  in Rust but private helpers may need `pub(crate)` or `pub(super)` widening — track each
  widening as a deliberate, reviewed surface change (the #520 pattern).
- **"pure" mislabel**: a helper named `*_for_active` may take `&self`; every pure claim
  is verified by reading the signature before the slice commits to a shared module.
- **Test fixtures coupled to the struct**: relocating tests may require exposing
  builders; keep these `#[cfg(test)]` and `pub(crate)`.
- **Rebase load on dependent PRs**: accepted by operator; early slices avoid the hottest
  regions (A4/A8 sequenced last among their tracks) to minimize churn before merge.
- **Re-export drift**: a re-export that outlives its need becomes a dual surface; each
  slice's final state removes the re-export only when all callers are migrated (tracked).

## Complexity Tracking

No behavior change accepted. Any slice that cannot be done as a pure behavior-preserving
move is split until it can, or escalated as a separate logic-change spec.
