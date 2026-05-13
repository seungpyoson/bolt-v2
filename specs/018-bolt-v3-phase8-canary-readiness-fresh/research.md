# Phase 8 Research Notes

## Fresh-main Anchor

- Worktree: `.worktrees/018-bolt-v3-phase8-canary-readiness-fresh`
- Branch: `018-bolt-v3-phase8-canary-readiness-fresh`
- `HEAD == origin/main == d6f55774c32b71a242dcf78b8292a7f9e537afab`
- Baseline: `cargo test --lib` passed with 446 passed, 0 failed, 1 ignored.

## Zoom-out Module Map

- `src/bolt_v3_live_node.rs` builds `BoltV3LiveNodeRuntime`, resolves SSM secrets, maps adapters, registers clients/strategies, wires runtime capture, checks live canary gate, arms submit admission, then enters `LiveNode::run`.
- `src/bolt_v3_live_canary_gate.rs` validates `[live_canary]`, byte-bounds the readiness report read, parses readiness stages, and rejects before runner entry.
- `src/bolt_v3_submit_admission.rs` owns the Phase 6 shared admission state; it starts unarmed, arms exactly once from the gate report, enforces count cap and notional cap, and counts admitted order candidates.
- `src/bolt_v3_decision_evidence.rs` owns decision evidence writing before admission.
- `src/strategies/eth_chainlink_taker.rs` owns strategy decisions. Its submit helper records decision evidence, asks submit admission, then calls NT `self.submit_order`.
- `src/nt_runtime_capture.rs` owns passive capture around the NT runner. Phase 8 should record evidence references from this surface, not replace it.
- `config/live.local.example.toml` is an example only. Current fresh worktree does not contain gitignored `config/live.local.toml`.
- `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md` is current runtime contract evidence for updown, Chainlink, live canary gate, and fee blockers.

## Current-main Phase 8 Capabilities

- Phase 6 submit admission is present and tested.
- Live canary gate is present and runs before `LiveNode::run`.
- Strategy submit path is fenced to decision evidence -> submit admission -> NT submit.
- Runtime capture wiring exists around the runner.

## Current-main Phase 8 Gaps

- Phase 7 no-submit readiness producer is not present on current main.
- No tracked `config/live.local.toml` exists in the fresh worktree.
- No Phase 8 tiny canary evidence module exists.
- No ignored Phase 8 operator harness exists.
- No current strategy-input safety approval exists.
- No real no-submit readiness report exists in current main artifacts.

## Decision: Phase 8 Implementation Readiness Path

Phase 8 should not start with live-order machinery. It should start with a fail-closed canary preflight and redacted dry/no-submit evidence because current main lacks the Phase 7 report producer and live action remains blocked. The implementation path must be:

1. Local preflight object and tests.
2. Dry/no-submit canary evidence artifact and tests.
3. Ignored operator harness source-fenced to production runner only.
4. Verification and external review.
5. Stop before live order until exact approval.

## Rejected Alternatives

- **Port PR #320 wholesale**: rejected because it is stacked on closed PR #319 and assumes Phase 7 content that is not on current main.
- **Start from local Phase 7 branch**: rejected because user requires fresh branches from current main and no stale continuation.
- **Implement live order before strategy-input safety audit**: rejected by mandatory safety gate.
- **Use env vars for credentials in operator harness**: rejected; SSM remains only credential source.
- **Add Bolt-owned reconciliation/evidence synthesis**: rejected; NT owns reconciliation and reports.
- **Use mock venue proof as live readiness**: rejected; mock proof cannot validate real adapter auth, venue behavior, or NT reconciliation.
