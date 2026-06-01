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
# admission-request construction folds INTO existing src/bolt_v3_submit_admission.rs (rule #9)

src/bolt_v3_operator_artifacts/   # Track B: mod.rs (core-glue: json-io, error enum, constants, re-exports) + submodules
```

Naming is a reviewable proposal; the agnostic shared names follow the spec-023 contract
(typed-in/typed-out, no strategy/venue imports at the boundary).

## Slice Sequencing

Each row is one gated PR. Line ranges are current-main anchors (verified per-slice in
that slice's speckit before movement). "Touches in-flight" notes where another open PR
edits the same region; operator has directed those PRs rebase onto this work, so this
work leads — but early slices deliberately avoid the hottest regions to prove the
method before forcing rebases.

### Track A — strategy monolith

| Slice | Scope | Source anchor | Class | In-flight overlap |
|---|---|---|---|---|
| **A1** | Pure decision/sizing/EV/side-selection/uncertainty math → `bolt_v3_taker_signal.rs` (+ tests) | ~6797–7544 | pure-logic | none (clear of #520/#508/#507 source edits) — **first slice** |
| **A2** | Market selection + candidate snapshot construction (pure) → `selection.rs` | 407–482, 6419–6601 | pure-logic | none |
| **A3** | Order-book state + VWAP/slippage sizing → `bolt_v3_book_sizing.rs` (rule #9) | 493–777 | state-struct + pure | none |
| **A4** | Pricing state (reference/RV/lead-venue) → `bolt_v3_taker_pricing.rs` | 956–1666 | NT-actor-coupled state | #508 (864–974), #520 (835–955 SignedTradeFlow) — sequence after they land |
| **A5** | Exposure/recovery state machine → `exposure.rs` | 977–1258, 2415–2655 | state-struct | none direct |
| **A6** | Source-proof / replay / evidence derivation → `source_proof.rs` | 5931–6317 | pure-logic | none |
| **A7** | Config structs + parse/validate → `config.rs` (or archetype) | 86–403, 5557–5886 | pure-logic | #508 (5150+) |
| **A8** | Admission-request construction + valuation → `bolt_v3_submit_admission.rs` (rule #9; kill test-only dup :7546) | 4228–4400, 7546 | pure-logic | #507 (4216–4358), #510 — sequence after they land |
| **A9** | Split the 229 tests to mirror submodules; `mod.rs` left as struct + `DataActor` + glue | 7599–18205 | tests | trails A1–A8 |

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
