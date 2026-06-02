# Evidence Ledger — #522 monolith decomposition

Base: `origin/main` `2938bc6f`. One row per slice. A slice is **Resolved** only when:
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

Execution order = A1→A10 by internal dependency (#522 leads; in-flight PRs rebase per
the plan's Rebase Matrix). A2 is foundational (A4/A6/A9 consume the side type it homes).

| Item | Scope | Target | RED/GREEN | Diff = pure move | CI | Review | State |
|---|---|---|---|---|---|---|---|
| A1 | OutcomeSide-free math + numeric primitives | `bolt_v3_taker_signal.rs` + `bolt_v3_numeric.rs` | ✓ 5 pure-unit tests relocated & pass in new home (theta_scaler, 3× uncertainty_band, robust_sizing); `bolt_v3_numeric` +4 characterization tests; no net test loss | ✓ pure move — strategy diff is only 2 added `use` blocks + deletions; moved bodies verbatim + `pub` (+ `seconds_to_market_end` rename); codex code-review confirmed no behavior change | ✓ green, code head `1d1c0523` (PR #524, all lanes; 1 disk-pressure rerun) | codex: plan SHIP + code-review clean (no behavior blocker); relays grok/glm/deepseek/kimi/gemini pending | code-resolved (strategy 18,205→17,931, −274); awaiting 6-model unanimous + operator merge permission |
| A2 | consolidate `OutcomeSide` → market-family (partially resolves #13: OutcomeSide) + side-using math | `bolt_v3_market_families/` + `bolt_v3_taker_signal.rs` | — | — | — | — | planned |
| A3 | market selection (pure; completes #13: CandidateMarket wrapper) | `…/selection.rs` | — | — | — | — | planned |
| A4 | book state + VWAP/slippage sizing | `bolt_v3_book_sizing.rs` | — | — | — | — | planned |
| A5 | pricing state | `bolt_v3_taker_pricing.rs` | — | — | — | — | planned (#520/#508 rebase onto it) |
| A6 | exposure/recovery state machine | `…/exposure.rs` | — | — | — | — | planned (#507 rebases) |
| A7 | source-proof / replay | `…/source_proof.rs` | — | — | — | — | planned |
| A8 | config parse/validate | `…/config.rs` | — | — | — | — | planned (#508 rebases) |
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
