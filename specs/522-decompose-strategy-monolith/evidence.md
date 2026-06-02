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
| A1 | OutcomeSide-free math + numeric primitives | `bolt_v3_taker_signal.rs` + `bolt_v3_numeric.rs` | ✓ 5 pure-unit tests relocated & pass in new home (theta_scaler, 3× uncertainty_band, robust_sizing); `bolt_v3_numeric` +4 characterization tests; no net test loss | ✓ pure move — strategy diff is only 2 added `use` blocks + deletions; moved bodies verbatim + `pub` (+ `seconds_to_market_end` rename); codex code-review confirmed no behavior change | ✓ green @ final head `b631268f` (PR #524, all 20 checks; head `1d1c0523`→`2fa9209b` docs→`b631268f` visibility fix) | **unanimous 6/6 APPROVE** @ `b631268f` (codex + grok/glm/deepseek/kimi/gemini); GPT's lone visibility BLOCK cleared by `pub`→`pub(crate)` tightening (no external/tests importer, no `pub use` re-export, all items retain in-crate callers so no dead_code; 11 allowlist contexts repointed; `#488 maker` doc overclaim dropped) | **RESOLVED — MERGED to main 2026-06-02 via merge commit `ccd38213`** (strategy 18,205→17,931, −274; merge-commit preserves `b631268f` so stacked A2 diff auto-narrows) |
| A2 | consolidate `OutcomeSide` → market-family (partially resolves #13: OutcomeSide) + side-using math | `bolt_v3_market_families/mod.rs` + `bolt_v3_taker_signal.rs` | ✓ 5 pure-unit tests relocated (worst-case-EV fail-closed + 4× side-selection) & pass in new home via CI nextest; no net test loss (strategy −5, taker_signal +5) | ✓ pure move — canonical `OutcomeSide` homed in `market_families/mod.rs`; updown `UpdownOutcomeSide` (6 refs) + strategy `OutcomeSide` (93 refs) collapsed by import; 5 fns/structs relocated verbatim + `pub(crate)`; 4 runtime-literal allowlist entries repointed; 2 strategy `use` blocks added | ✓ code-final CI green @ `1f6e99f5` (after `pub`→`pub(crate)` visibility tighten `ce95b4af`; clippy/deny/fmt/source-fence/nextest×4+archive/aarch64/gate all pass; build/deploy/same-sha skip = main-only lanes); this commit = doc-only ledger/slice refresh (no code delta) — exact-head CI re-confirmed before merge | codex: plan approve + code-review **approve** (re-ran fmt + all source-fences + relocated `task4_` tests); 5-lens adversarial workflow **5/5 pass, 0 findings** (verbatim-move, type-identity, visibility/imports, fence/allowlist, completeness/scope); gemini-code-assist PR comment dispositioned (proposed `is_finite()` guard = out-of-scope logic change, pre-existing & domain-unreachable; tracked as class-level hardening); **first-round 6-model relay**: gemini/kimi/deepseek/glm **APPROVE** (0 findings); grok **APPROVE + 2 doc findings**; GPT **BLOCK** — all on the same in-scope doc staleness (evidence/A2.md said `pub` + cited pre-tighten `6ef731fa`; head is `pub(crate)` via `ce95b4af`), **no Rust/behavior issue found**; **FIXED here** by refreshing docs to `pub(crate)` + final head `1f6e99f5` (no-op "drop OutcomeSide-free" bullet removed; speculative `#488 maker` reuse clause dropped) | code-resolved (strategy 17,931→17,731, −200); awaiting delta re-relay confirm + operator merge permission |
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
