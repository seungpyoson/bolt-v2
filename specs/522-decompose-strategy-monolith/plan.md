# Implementation Plan: Decompose the bolt-v3 monoliths

**Branch**: `refactor/522-decompose-strategy-monolith` | **Spec**: `./spec.md` | **Tracking**: #522

## Summary

Behavior-preserving decomposition of `binary_oracle_edge_taker.rs` (18,205 lines) and
`bolt_v3_operator_artifacts.rs` (17,466 lines) into focused modules and intended
shared helpers, slice by slice, each behind the per-slice gate in `spec.md`. The
strategy file converges to a directory module: `strategies/binary_oracle_edge_taker/`
with `mod.rs` (struct + `DataActor` orchestration + intent/signal glue) plus
submodules; genuinely shared, agnostic math/state moves to `src/` shared modules the
#488 maker reuses.

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
  tests/            # the 229 tests, split to mirror submodules

src/bolt_v3_taker_signal.rs   # SHARED: pure decision/sizing/EV/side-selection/uncertainty math (maker-reusable)
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
operator direction **#522 LEADS**: the in-flight PRs (#507/#510/#520/#508) are NOT
prerequisites here — they rebase onto each merged slice (see the Rebase Matrix). The
last column names which in-flight PR must rebase after a slice lands. Line ranges are
current-main anchors, re-verified per-slice before movement.

### Track A — strategy monolith

| Slice | Scope | Source anchor | Class | Rebases onto it |
|---|---|---|---|---|
| **A1** | **OutcomeSide-free** pure math → new `bolt_v3_taker_signal.rs`; generic numeric primitives → existing `bolt_v3_numeric.rs` | consts 6817–6819; fns 6875–6929, 7017–7053, 7119–7147; structs 7009, 7034, 7110 | pure-logic | — (first slice; no overlap) |
| **A2** | Consolidate `OutcomeSide` into the market-family layer (merge with `UpdownOutcomeSide`; **resolves findings-doc #13**); move the side-using math (`compute_worst_case_ev_bps`+`WorstCaseEvInputs`, `choose_entry_side`+`SideSelectionInputs`, `outcome_side_evidence_label`) into `bolt_v3_taker_signal` depending on that owner | 6883–6894, 7026–7032, 7056–7108; 93 refs repointed | cross-cutting type move | — |
| **A3** | Market selection + candidate snapshot construction (pure) → `selection.rs` | 407–482, 6419–6601 | pure-logic | — |
| **A4** | Order-book state + VWAP/slippage sizing → `bolt_v3_book_sizing.rs` (rule #9) | 493–777 | state-struct + pure | — |
| **A5** | Pricing state (reference/RV/lead-venue) → `bolt_v3_taker_pricing.rs` | 956–1666 | NT-actor-coupled state | #520 (SignedTradeFlow 835–955), #508 (864–974) |
| **A6** | Exposure/recovery state machine → `exposure.rs` | 977–1258, 2415–2655 | state-struct | #507 (sizer evidence on position state) |
| **A7** | Source-proof / replay / evidence derivation → `source_proof.rs` | 5931–6317 | pure-logic | — |
| **A8** | Config structs + parse/validate → `config.rs` (or archetype) | 86–403, 5557–5886 | pure-logic | #508 (config guards 5150+) |
| **A9** | Admission-request construction + valuation → `bolt_v3_submit_admission.rs` (rule #9; kill test-only dup :7546). **Owns the base — #507/#510 rebase their admission edits onto it.** | 4228–4400, 7546 | pure-logic | #507 (+1946), #510 (+134) |
| **A10** | Split the 229 tests to mirror submodules; `mod.rs` = struct + `DataActor` + glue | 7599–18205 | tests | — |

### Track B — operator_artifacts (parallel, conflict-free with Track A)

One directory module; ~13 extractable concern-modules (gate-evidence, ssm-manifest/
redaction, data-client-readiness, financial-envelope/approval-nonce,
market-selection-source, abort-plan-proof, strategy-input-evidence, chainlink-streams,
entry-decision-source, live-canary-terminal/secret-scan); core-glue (json-io, the
70-variant error enum, the 200+ constants) stays in `mod.rs`. Public API re-exported so
`tests/bolt_v3_operator_artifacts.rs` (in-flight #507/#510) stays green. Sliced into a
handful of gated PRs by concern.

### Wave-2 shared-layer cleanups (after A8 lands)

canary-proof claim decoupling from shared admission (#502); `polymarket_*`→`market_*`
evidence rename (finding #12); provider credential/HTTP dedup (#447) + CLOB-v2
vendor-type relocation + fee-provider coupling (#446); live-node probe-orchestration
extraction. Tracked here, planned when their prerequisite slices land.

## Rebase Matrix (#522 leads; in-flight PRs rebase onto merged slices)

Resolves the ordering ambiguity: the in-flight PRs are **not** prerequisites for any
#522 slice. Each rebases its edits onto the relevant slice **after** that slice merges.
The admission slice (A9) deliberately owns the base for the rule-#9 region so the
heavy admission editors rebase onto the cleaned module — not the reverse.

| In-flight PR | Region it edits | Rebases after | What it rebases |
|---|---|---|---|
| #520 hoist SignedTradeFlow | strategy 835–955 | A5 | the hoist re-targets the extracted pricing/trade-flow module |
| #508 causality/config hardening | strategy 864–974, 5150+ | A5, A8 | guards re-applied on the extracted pricing + config modules |
| #507 position-sizer | `submit_admission` +1946, `decision_evidence` +538, strategy 4216–4358 | A9 (and A6) | admission/evidence integration re-applied on the extracted admission-request module |
| #510 loss-governor | `submit_admission` +134, `live_node`, `decision_evidence` | A9 | admission rejection path re-applied on the extracted module |

If product priorities require an in-flight PR to merge *before* its dependent slice,
that slice instead rebases onto the PR — but the default, per operator direction, is
#522-leads. Either way there is exactly one deterministic base at any time.

## Per-Slice Method (RED → GREEN → move → verify)

1. **Characterize**: read the exact unit at current HEAD; confirm `&self`-freedom for
   "pure" claims; enumerate inbound callers and the tests that cover it.
2. **RED**: add/relocate a characterization test pinning current behavior; show it
   discriminates (fails against a wrong control) — recorded in the ledger.
3. **Move**: relocate the unit + its tests to the target module; `pub use` re-export
   from the origin so no external call site changes. No signature/logic change.
4. **GREEN**: `cargo fmt`/`clippy`/targeted tests local; push; CI runs full suite.
5. **Verify**: line count of the source monolith strictly decreases; the diff is a pure
   move + re-export (`git diff` shows no logic delta); ledger item marked resolved with
   anchors.

## Risks

- **Inherent-impl spread**: splitting `impl BinaryOracleEdgeTaker` across files is legal
  in Rust but private helpers may need `pub(crate)` or `pub(super)` widening — track each
  widening as a deliberate, reviewed surface change (the #520 pattern).
- **"pure" mislabel**: a helper named `*_for_active` may take `&self`; every pure claim
  is verified by reading the signature before the slice commits to a shared module.
- **Test fixtures coupled to the struct**: relocating tests may require exposing
  builders; keep these `#[cfg(test)]` and `pub(crate)`.
- **Rebase load on in-flight PRs**: accepted by operator; early slices avoid the hottest
  regions (A4/A8 sequenced last among their tracks) to minimize churn before merge.
- **Re-export drift**: a re-export that outlives its need becomes a dual surface; each
  slice's final state removes the re-export only when all callers are migrated (tracked).

## Complexity Tracking

No behavior change accepted. Any slice that cannot be done as a pure behavior-preserving
move is split until it can, or escalated as a separate logic-change spec.
