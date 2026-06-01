# External Review Packet: Production Kill Switch Design

## Review Scope

Review the design-only goal package for a future production-grade kill switch system in `bolt-v2`.

Primary files:

- `goals/production-kill-switch/facts.md`
- `goals/production-kill-switch/research.md`
- `goals/production-kill-switch/design.md`
- `goals/production-kill-switch/plan.md`

Supporting current repo evidence:

- `specs/505-nt-loss-governor/spec.md`
- `specs/505-nt-loss-governor/plan.md`
- `src/bolt_v3_submit_admission.rs`
- `src/bolt_v3_live_node.rs`
- `src/bolt_v3_strategy_registration.rs`
- `goals/production-kill-switch/source-excerpts/binary-oracle-edge-taker-submit-path.md`
- `docs/bolt-v3/2026-04-28-source-grounded-status-map.md`
- `docs/bolt-v3/research/runtime-capture/nt-msgbus-surfaces.yaml`
- `scripts/verify_bolt_v3_strategy_policy_fence.py`
- pinned NautilusTrader local source at `/Users/spson/.cargo/git/checkouts/nautilus_trader-3c6af4345b4d438b/6e059dc`

## Required Verdict Format

Return one of:

- `APPROVE`: no blocking findings for creating the GitHub issue from this design.
- `REQUEST_CHANGES`: at least one blocking design flaw must be fixed before issue creation.

Findings-first. For every blocking finding, cite exact file/line evidence where possible and explain the concrete risk.

## Approval Criteria

Approve only if the design:

1. Matches the accepted facts in `facts.md`.
2. Keeps this setup goal design-only and avoids production implementation claims.
3. Targets a global bolt-v3 LiveNode/runtime kill switch, not strategy-local logic.
4. Covers detect, latch, stop new risk, cancel, forced-reduction flatten, verify, and authorized manual reset.
5. Uses NT-native state and APIs only; no bespoke venue cancel/flatten path.
6. Correctly handles NT `TradingState::Reducing` versus `TradingState::Halted`.
7. Requires durable halt/reset evidence and fail-closed restart behavior.
8. Requires a distinct proof-bound forced-reduction admission path so ordinary submit caps cannot deadlock flattening.
9. Requires cancel and reconciliation coverage for open, inflight, pending-cancel, emulated, algorithm-managed, contingent, and accepted-but-not-terminal order risk.
10. Requires reconciliation proof before claiming flat, with mandatory proof streams failing closed when missing/stale/contradictory.
11. Requires authorized, tamper-evident manual reset before returning to armed trading.
12. Includes source fences and tests broad enough to stop bypasses.
13. Separates PR #480-dependent live wiring from PR-independent design/state/config work.
14. Produces a single phased GitHub issue after required model approval.

## Current Known Facts

- PR #480 was verified with `gh pr view 480` on 2026-06-01: open, draft, merge-clean, base `535c0e858c291b066775c15305521e6883ed5b3c`, and head `25d0e32bdaf09eac52621d2d9eda6f6703aac1a7`.
- PR #480 checks were green at that verification time, with expected skipped jobs `same-sha-main-evidence` and `deploy`.
- Current loss-governor work is admission-only and explicitly excludes cancel/flatten.
- Pinned NT revision is `6e059dcbb59ac1e582132fc431a581936c216c3c`.
- Pinned NT exposes `RiskEngine::set_trading_state`.
- Pinned NT `TradingState::Halted` denies submits, while `TradingState::Reducing` allows reducing submits and rejects exposure-increasing submits.
- NT cache exposes open-order and open-position query APIs.
- NT cache and strategy helpers also expose inflight, pending-cancel, emulated, and algorithm-managed order surfaces relevant to cancellation/reconciliation.
- NT strategy APIs expose cancel and close-all-position paths.

## Review Question

Is this design strong enough to create a GitHub issue for a production-grade kill switch system, or does it need changes first?
