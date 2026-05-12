# Polymarket No-Submit Readiness Approval Package

Date: 2026-05-12
Branch base: `origin/codex/bolt-v3-order-lifecycle-proof` at `a0f78627aa05f2f5fda0fa48e6c3ef917eb4694b`

## Recommendation

Approve only a **no-submit authenticated Polymarket readiness slice**.

Goal: prove bolt-v3 can resolve SSM secrets, construct the pinned NT Polymarket execution client, authenticate/connect, refresh private account state, observe account/user-channel readiness, and stop cleanly.

This is not live trading approval. This is not canary approval. This must not place, cancel, replace, or amend orders.

## Current Evidence

Local/source proof already exists:

- F8: order admission is locally proven above NT for the currently identified existing-strategy admission reasons. Evidence is summarized in `docs/bolt-v3/2026-05-10-bolt-v3-follow-up-tracker.md`.
- F9: local submit/reject/fill/exit/cancel-shaped proof exists through NT `LiveNode::run`, mock execution, and a local CLOB fixture. Evidence: `tests/bolt_v3_order_lifecycle_tracer.rs`.
- F9 boundary: external Polymarket execution is authenticated/private infrastructure, not a public no-capital path. Evidence: `tests/bolt_v3_polymarket_external_execution_boundary.rs`.
- F10: local restart/reconciliation proof exists with mock mass status and source-verified NT reconciliation behavior. Evidence: `tests/bolt_v3_reconciliation_restart.rs`.

Fresh local verification on 2026-05-12:

- `cargo test --test bolt_v3_polymarket_external_execution_boundary -- --test-threads=1` passed 3/3.
- `cargo test --test bolt_v3_order_lifecycle_tracer -- --test-threads=1` passed 5/5.
- `cargo test --test bolt_v3_reconciliation_restart -- --test-threads=1` passed 6/6.

## Missing Evidence

Still unproven:

- Real authenticated Polymarket execution connect under bolt-v3.
- Real private account-state refresh under bolt-v3.
- Real user-channel subscription/ready behavior under bolt-v3.
- Real fee/balance/account readiness under bolt-v3.
- Real disconnect/stop behavior after authenticated connect.
- Real venue reconciliation with Polymarket after private connect.
- Any live submit/cancel/fill behavior.

## Allowed Scope

Allowed in this slice:

- Load existing bolt-v3 TOML.
- Resolve secrets only through SSM via existing Rust SSM path.
- Build NT `LiveNode`.
- Register NT Polymarket execution client through existing v3 mapping.
- Connect authenticated execution client.
- Observe user-channel/account-state readiness.
- Query/read fee, balance, allowance, and account readiness if NT already does so during connect.
- Stop/disconnect cleanly.
- Persist a readiness report containing only redacted secret metadata and non-secret status.

## Forbidden Scope

Forbidden in this slice:

- `submit_order`
- `cancel_order`
- `cancel_all_orders`
- strategy start that can emit order intents
- market-making, taker, canary, or dry-run order path
- Python runtime or Python sidecar
- direct venue REST/WebSocket calls outside NT
- environment variable secret fallback
- hardcoded wallet, market, venue, strategy, quantity, timeout, slug, or feed values
- changing strategy policy
- changing order admission policy
- changing reconciliation behavior

## Required Guard

Before any authenticated external run, add a source/test guard proving the no-submit runner cannot call order APIs.

Minimum guard surface:

- Source scan on the no-submit runner for `submit_order`, `cancel_order`, `cancel_all_orders`, `TradingCommand::SubmitOrder`, `TradingCommand::CancelOrder`, and `TradingCommand::CancelAllOrders`.
- Source scan proving the runner does not register strategies or start strategy actors.
- Test proving missing SSM secrets fail closed before connect.
- Test proving resolved secret values are redacted from readiness output.

## Acceptance Criteria

Accepted evidence for this slice:

- A committed no-submit runner or test harness that uses the same v3 TOML/SSM/config path as production.
- Tests proving forbidden order APIs are unreachable from that runner.
- Tests proving no strategy is registered for the no-submit readiness run.
- Tests proving no Python path is involved.
- Redacted readiness artifact from one approved authenticated Polymarket run.
- The run shows connect/account-state/user-channel readiness or a concrete NT/venue error.
- The run stops cleanly.

Unacceptable evidence:

- Local mock-only proof.
- Python script output.
- Direct Polymarket REST/WebSocket probe outside NT.
- Any live order, cancel, amend, replace, fill, or position mutation.
- Any unredacted secret value in logs, reports, or docs.
- A runner that diverges from the production v3 config/SSM path.

## Approval Required

Separate explicit approval is required before:

- resolving real SSM production secrets,
- connecting to private Polymarket execution infrastructure,
- running from an operator host,
- allowing any submit/cancel/canary path.

This document requests approval only for the design of the no-submit readiness slice. It does not approve execution against real credentials.

## Next Implementation Slice

Slice name: `bolt-v3-polymarket-no-submit-readiness`

One PR scope:

- Add a no-submit authenticated readiness runner or ignored integration test.
- Add no-order source guards.
- Add redaction tests.
- Add runbook instructions for the manually approved external run.
- Do not execute against real credentials in CI.

Stop after this slice. If it passes, next decision is whether to approve a tiny canary design. If it fails, record the concrete NT/venue blocker and do not widen scope.
