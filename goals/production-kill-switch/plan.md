# Production Kill Switch Plan

## Solution Approach

Design a global bolt-v3 kill switch around the existing shared submit-admission boundary and pinned NautilusTrader risk/cache/order surfaces. The implementation issue should phase work from pure state/config/evidence first, then admission latch, then NT-native cancel/flatten/reconciliation, and finally no-submit drill proof. This goal stops after the external model approval quorum and GitHub issue creation.

Authoritative inputs:

- `goals/production-kill-switch/facts.md`
- `goals/production-kill-switch/research.md`
- `goals/production-kill-switch/design.md`

## Ordered Steps

1. Finalize design packet.
   - Files: `goals/production-kill-switch/facts.md`, `research.md`, `design.md`, `plan.md`.
   - Systems: Plannotator facts, local repo research, pinned NT source.
   - Verification: inspect accepted facts and ensure every accepted fact has design coverage.

2. Run the Plannotator workflow checkpoint.
   - Files: `goals/production-kill-switch/plan.md`.
   - Systems: `plannotator annotate --gate`.
   - Verification: Plannotator returns approved result or revisions are applied and rerun.
   - This is not the design approval gate; it only checks the goal package before external review.

3. Prepare model-review packet.
   - Files: `goals/production-kill-switch/review-packet.md`, `goals/production-kill-switch/review-commands.md`.
   - Systems: facts, research, design, plan, exact current repo/PR state.
   - Verification: packet names scope as design-only, names quorum as four of six with Claude and Gemini mandatory, asks reviewers for blocking findings only, and the runbook names the exact selected source-send approval text.

4. Run the external model design approval gate.
   - Systems: Relay Claude, Relay Gemini, Relay Kimi, Relay Grok, Relay DeepSeek, Relay GLM.
   - Required quorum: at least four of six approved, with Claude and Gemini included.
   - Verification: `goals/production-kill-switch/reviews.md` records provider, verdict, blocking findings, and any design changes made. Any blocking finding requires revision and re-review of the changed packet.
   - This is the design approval gate and must pass before issue creation.
   - Current status: not approved; `goals/production-kill-switch/reviews.md` records zero accepted external approvals and the sandbox rejection of source-bearing review launches even after exact user approval.

5. Create the GitHub issue.
   - Files: `goals/production-kill-switch/issue-draft.md`, `goals/production-kill-switch/issue.md`.
   - Systems: GitHub CLI.
   - Issue shape: one phased issue containing approved design, production invariants, implementation phases, forced-reduction admission requirements, full outstanding-order reconciliation requirements, verification matrix, review evidence, and PR #480 dependencies.
   - Verification: external model gate has passed, GitHub issue is created, and issue URL is recorded in `goals/production-kill-switch/issue.md`.

6. Write final goal artifact.
   - Files: `goals/production-kill-switch/goal.md`.
   - Systems: Plannotator goal package.
   - Verification: `goal.md` references `facts.md`, `plan.md`, approved reviews, and the created issue URL.

## Future Implementation Files And Systems

The GitHub issue should propose these future implementation targets:

- `src/bolt_v3_kill_switch.rs`: pure state machine, trigger model, action model, forced-reduction model, reconciliation model.
- `src/bolt_v3_kill_switch_store.rs`: durable halt/reset evidence store.
- `src/bolt_v3_config.rs` and `src/bolt_v3_validate.rs`: `[risk.kill_switch]` TOML parsing and validation.
- `src/bolt_v3_submit_admission.rs`: global halt latch enforcement before NT submit and proof-bound forced-reduction admission.
- `src/bolt_v3_live_node.rs`: runtime wiring, NT risk state integration, and selected action-routing boundary.
- `src/bolt_v3_strategy_registration.rs`: pass shared global runtime handles and optional per-strategy action ports without strategy-local kill policy.
- `src/bolt_v3_order_intent.rs`: reuse typed NT order construction for flatten orders.
- `src/bolt_v3_decision_evidence.rs`: kill-switch event and admission evidence.
- `scripts/verify_bolt_v3_kill_switch_fence.py`: source fence for bypasses and strategy-local kill logic.
- Tests: state machine, durable-store failure, config parsing, submit admission, forced-reduction cap behavior, live node wiring, all outstanding-order cancel races, flatten reconciliation, reset authorization, restart recovery, no-submit drill.
- Docs: runbook, quickstart, production readiness status map update.

## Verification For Future Implementation

- `cargo test --locked --lib bolt_v3_kill_switch`
- `cargo test --locked --test config_parsing`
- `cargo test --locked --test bolt_v3_submit_admission`
- `cargo test --locked --test bolt_v3_kill_switch_reconciliation`
- `cargo test --locked --test bolt_v3_live_node`
- `cargo fmt --check`
- `cargo clippy --locked --lib -- -D warnings`
- `cargo clippy --locked --bin bolt-v2 -- -D warnings`
- `just source-fence`
- no-submit kill drill command added by the implementation issue

## Risks And Open Questions

- PR #480 may change the order-intent/admission boundary. Live wiring must be rebased after #480 lands on `main`.
- Cancel/flatten routing must be proven before implementation. A standalone NT action actor is not enough if it cannot act across the configured strategy/account scope; acceptable routes are per-strategy action ports orchestrated globally or a live-node command router with identity-preserving NT command routing.
- `TradingState::Reducing` is useful during flatten, while `TradingState::Halted` denies submits. Tests must prove the chosen state sequence does not block required flatten orders.
- Ordinary submit-admission count and notional caps can block risk-reducing exits. The design therefore requires a separate proof-bound forced-reduction path for kill-switch flattening, with TOML-owned policy and tests for normal-cap exhaustion.
- "No open orders" is too narrow for flat proof. Reconciliation must cover open, inflight, pending-cancel, emulated, algorithm-managed, contingent, and accepted-but-not-terminal order risk.
- NT `subscribe_positions` is not a proof source on the pinned revision because no publisher was found. Reconciliation must use cache/portfolio state and captured position events.
- Durable store corruption/missing evidence and state-write/fsync failure must fail closed, which can make recovery operationally strict. The runbook must make manual intervention explicit.
- Manual reset must be authorized and tamper-evident; reset evidence alone is not sufficient to re-arm trading.
