# Evidence Ledger — #522 monolith decomposition

Base refreshed for A6 branch: `origin/main` `da7247f0`. One row per slice. A slice is **Resolved** only when:
move PR landed behind the gate · monolith line count strictly decreased · diff is a
pure relocation — move + imports, with a `pub use` re-export only where an external
caller requires it and **none** for private-internal slices like A1 (no logic delta) ·
RED/GREEN recorded · exact-head CI green · unanimous 6-model review (or recorded
waiver) · operator merge permission.

## Baselines (current-main, to be re-confirmed at each slice HEAD)

| File | Lines @ 2938bc6f | Tests |
|---|---:|---:|
| `src/strategies/binary_oracle_edge_taker.rs` | 18,205 | 229 |
| `src/bolt_v3_operator_artifacts.rs` | 17,466 | 5 |

## Track A — strategy monolith

Execution order = A1→A10 by internal dependency (#522 leads; open dependent PRs rebase
per the plan's Rebase Matrix). A2 is foundational (A4/A6/A9 consume the side type it
homes).

| Item | Scope | Target | RED/GREEN | Diff = pure move | CI | Review | State |
|---|---|---|---|---|---|---|---|
| A1 | OutcomeSide-free math + numeric primitives | `bolt_v3_taker_signal.rs` + `bolt_v3_numeric.rs` | ✓ 5 pure-unit tests relocated & pass in new home (theta_scaler, 3× uncertainty_band, robust_sizing); `bolt_v3_numeric` +4 characterization tests; no net test loss | ✓ pure move — strategy diff is only 2 added `use` blocks + deletions; moved bodies verbatim + `pub` (+ `seconds_to_market_end` rename); codex code-review confirmed no behavior change | ✓ green @ final head `b631268f` (PR #524, all 20 checks; head `1d1c0523`→`2fa9209b` docs→`b631268f` visibility fix) | **unanimous 6/6 APPROVE** @ `b631268f` (codex + grok/glm/deepseek/kimi/gemini); GPT's lone visibility BLOCK cleared by `pub`→`pub(crate)` tightening (no external/tests importer, no `pub use` re-export, all items retain in-crate callers so no dead_code; 11 allowlist contexts repointed; `#488 maker` doc overclaim dropped) | **RESOLVED — MERGED to main 2026-06-02 via merge commit `ccd38213`** (strategy 18,205→17,931, −274; merge-commit preserves `b631268f` so stacked A2 diff auto-narrows) |
| A2 | consolidate `OutcomeSide` → market-family (partially resolves #13: OutcomeSide) + side-using math | `bolt_v3_market_families/mod.rs` + `bolt_v3_taker_signal.rs` | ✓ 5 pure-unit tests relocated (worst-case-EV fail-closed + 4× side-selection) & pass in new home via CI nextest; no net test loss | ✓ pure move — canonical `OutcomeSide` homed in `market_families/mod.rs`; side-using math relocated verbatim to `bolt_v3_taker_signal.rs`; runtime-literal allowlist repointed | ✓ merged behind PR #526 gate at merge commit `a2461169` | codex + external review gate completed before operator merge | **RESOLVED — MERGED to main via PR #526** |
| A3 | market selection (pure; completes #13: CandidateMarket wrapper) | `…/selection.rs` | ✓ landed with source-root parity updates and strategy directory conversion | ✓ pure move of selection cluster; `selection.rs` exists at current main | ✓ merged on main at `2e3ef7a7` | historical review packet not re-audited in A6 branch | **RESOLVED — MERGED to main** |
| A4 | book state + VWAP/slippage sizing | `bolt_v3_book_sizing.rs` | ✓ relocated book-sizing tests / source-integrity proof with shared module | ✓ `src/bolt_v3_book_sizing.rs` added; strategy imports shared book state; no strategy re-export | ✓ merged on main at `dab298e8` | historical review packet not re-audited in A6 branch | **RESOLVED — MERGED to main** |
| A5 | pricing state | `bolt_v3_taker_pricing.rs` | ✓ taker-pricing tests added in `tests/bolt_v3_taker_pricing.rs` | ✓ `src/bolt_v3_taker_pricing.rs` added; strategy pricing state imports shared module | ✓ merged behind PR #572 at `607d23a9` | historical review packet not re-audited in A6 branch | **RESOLVED — MERGED to main** |
| A6 | exposure/recovery state machine | `…/exposure.rs` | ✓ wrong-control RED failed as expected; restored GREEN targeted test passed (`1 passed; 553 filtered out`) with operator-approved alternate root `<ALTERNATE_VERIFICATION_ROOT>` | local WIP pure move: `exposure.rs` added; `mod.rs` definitions removed; no `pub use`; source-integrity golden re-derived with dynamic file walk; `git diff --check`, `cargo fmt --check`, lib/bin clippy, and `just source-fence` green locally | PR #583 CI green at the code head; final exact-head CI must be rechecked after review-comment fixes | internal adversarial review complete locally; external review packet pending user relay after PR comments are addressed | **IN PROGRESS — not resolved** (`mod.rs` 17,480→17,127 locally; `exposure.rs` 416 lines; PR/review/merge still pending) |
| A7 | source-proof / replay | `…/source_proof.rs` | — | — | — | — | planned |
| A8 | config parse/validate | `…/config.rs` | ✓ config parse/validate moved with source-root parity checks | ✓ `config.rs` exists; strategy imports builder/config surface from submodule | ✓ merged behind PR #560 at merge commit `52c83924` | historical review packet not re-audited in A6 branch | **RESOLVED — MERGED to main** |
| A9 | admission-request construction (kill dup :7546) — owns base | `bolt_v3_submit_admission.rs` | — | — | — | — | planned (#507/#510 rebase onto it) |
| A10 | split 229 tests; mod.rs = struct + DataActor + glue | `…/tests/` | — | — | — | — | planned (trails A1–A9) |

## Track B — operator_artifacts

| Item | Scope | Target | State |
|---|---|---|---|
| B1–Bn | concern-modules (gate-evidence, ssm-manifest, data-client-readiness, financial-envelope, market-selection-source, abort-plan-proof, strategy-input-evidence, chainlink-streams, entry-decision-source, live-canary-terminal) | `bolt_v3_operator_artifacts/` | planned (parallel) |

## Wave-2 shared-layer (after A8)

| Item | Scope | Tracked-by | State |
|---|---|---|---|
| W2-1 | canary-proof claim decoupling from shared admission | #502 | planned |
| W2-2 | `polymarket_*`→`market_*` evidence rename | finding #12 | planned |
| W2-3 | provider credential/HTTP dedup + CLOB-v2 relocation + fee-provider coupling | #447 / #446 | planned |
| W2-4 | live-node probe-orchestration extraction | — | planned |
