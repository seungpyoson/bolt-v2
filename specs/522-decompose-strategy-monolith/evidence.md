# Evidence Ledger — #522 monolith decomposition

Base: `origin/main` `2938bc6f`. One row per slice. A slice is **Resolved** only when:
move PR landed behind the gate · monolith line count strictly decreased · diff is a
pure move+re-export (no logic delta) · RED/GREEN recorded · exact-head CI green ·
unanimous 6-model review (or recorded waiver) · operator merge permission.

## Baselines (current-main, to be re-confirmed at each slice HEAD)

| File | Lines @ 2938bc6f | Tests |
|---|---:|---:|
| `src/strategies/binary_oracle_edge_taker.rs` | 18,205 | 229 |
| `src/bolt_v3_operator_artifacts.rs` | 17,466 | 5 |

## Track A — strategy monolith

| Item | Scope | Target | RED/GREEN | Diff = pure move | CI | Review | State |
|---|---|---|---|---|---|---|---|
| A1 | pure decision/sizing/EV math (~6797–7544) | `bolt_v3_taker_signal.rs` | — | — | — | — | planned |
| A2 | market selection (pure) | `…/selection.rs` | — | — | — | — | planned |
| A3 | book state + VWAP/slippage sizing | `bolt_v3_book_sizing.rs` | — | — | — | — | planned |
| A4 | pricing state | `bolt_v3_taker_pricing.rs` | — | — | — | — | planned (after #508/#520) |
| A5 | exposure/recovery state machine | `…/exposure.rs` | — | — | — | — | planned |
| A6 | source-proof / replay | `…/source_proof.rs` | — | — | — | — | planned |
| A7 | config parse/validate | `…/config.rs` | — | — | — | — | planned (after #508) |
| A8 | admission-request construction (kill dup :7546) | `bolt_v3_submit_admission.rs` | — | — | — | — | planned (after #507/#510) |
| A9 | split 229 tests; mod.rs = struct + DataActor + glue | `…/tests/` | — | — | — | — | planned (trails A1–A8) |

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
