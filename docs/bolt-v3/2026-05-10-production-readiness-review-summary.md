# Bolt-v3 Production Readiness Review Summary

Date: 2026-05-10

Source of truth for evidence: `docs/bolt-v3/2026-05-10-production-readiness-evidence-ledger.md`

## Initial Ask

The request was to stop producing shallow or speculative answers about bolt-v3 and answer, with hard evidence, what it takes to get from the current bolt-v2 state to a production-grade bolt-v3.

The target state was:

- bolt-v3 lives inside `bolt-v2`, not as a separate product.
- NautilusTrader remains the intended Rust production runtime.
- The system should eventually deploy many strategies concurrently across many markets and venues.
- The review should use issues, docs, PRs, local code, and NT source.
- The review should be read-only and evidence-backed.
- PR #300 should not be treated as accepted evidence.

## What I Did

I reviewed the current bolt-v2 state from these surfaces:

- Current branch and remote state, including `origin/main`.
- Existing bolt-v3 docs, especially the source-grounded status map and runtime contracts.
- The newly drafted production-readiness evidence ledger.
- Prior GitHub issues and PRs around bolt-v3 scope, provider neutrality, live readiness, NT integration, and production gates.
- Local bolt-v3 Rust code for config loading, SSM secret resolution, adapter mapping, provider bindings, market-family bindings, archetype validation, LiveNode construction, readiness, and controlled connect.
- Pinned NautilusTrader source for `LiveNode`, `LiveNodeBuilder`, `Strategy`, trader registration, risk, execution, reconciliation, cache, persistence, backtest, and Polymarket adapter behavior.

I also ran focused local checks earlier in the investigation:

- `cargo test --test bolt_v3_readiness -- --nocapture`
- `cargo test --test bolt_v3_controlled_connect -- --nocapture`
- `python3 scripts/verify_bolt_v3_runtime_literals.py`
- `python3 scripts/verify_bolt_v3_provider_leaks.py`

Those checks passed when run, but the evidence ledger intentionally does not upgrade production claims based only on local tests.

## Main Finding

The problem is not that NautilusTrader lacks a production runtime. NT has real live-node, strategy, risk, execution, reconciliation, backtest, cache, persistence, and Polymarket adapter machinery.

The production gap is that bolt-v3 has not yet built and proven the product layer that turns bolt-v3 TOML into real NT strategy runtime behavior with activation, evidence, risk, orders, reconciliation, and deployment gates.

Put plainly:

- NT can run production trading systems.
- bolt-v3 currently proves only foundation/plumbing pieces.
- bolt-v3 does not yet prove a supervised live-trading transition.

## Current Accepted Shape

Bolt-v3 should stay NT-first in Rust for production, but the path needs to become thinner and more vertical.

The production path should be:

1. Load explicit v3 config.
2. Resolve secrets from SSM.
3. Map venues/providers into NT clients.
4. Prove target instruments are present and tradable.
5. Construct real NT strategies from strategy configs.
6. Register strategies before NT starts.
7. Persist fixed decision events before submit.
8. Submit through NT risk/execution.
9. Capture fills/rejects/positions.
10. Reconcile on restart.
11. Promote only through dry-run, shadow, tiny canary, and release gates.

## Recommended Verticals

1. Evidence ledger and source-of-truth discipline
   - Keep one factual table of claims, proof, missing proof, status, and next action.
   - This prevents verifier output or stale PRs from becoming fake architecture confidence.

2. Generic provider, market-family, and archetype boundaries
   - Core bolt-v3 should not bake in one provider, one market family, or one strategy shape.
   - Current genericity claims are unproven because static/root binding edits are still required.

3. Real bolt-v3 entrypoint inside bolt-v2
   - Operators need one authoritative v3 run/check path in the existing binary.
   - It must not be scattered test helpers or legacy run behavior.

4. NT strategy construction and registration
   - TOML strategy configs must become real NT `Strategy` objects.
   - This is the first major live-trading blocker.

5. Backtest/live parity
   - The same strategy logic must work in backtest and live.
   - Current bolt-v3 is live-mode only, so parity claims should be rejected until implemented.

6. Instrument readiness and activation
   - Connecting clients is not enough.
   - Bolt-v3 must prove the selected market instruments are loaded, current, and tradable before strategies activate.

7. Reference data and facts
   - Strategies need clean facts for prices, anchors, fees, tick sizes, limits, and venue constraints.
   - These should not be scraped or guessed inside strategy code.

8. Risk, sizing, and execution gates
   - Bolt-v3 must prove size, exposure, cooldown, kill switch, and risk constraints before any order reaches the venue.

9. Decision-event persistence
   - Every decision needs durable evidence before submit: inputs, signal, order intent, risk result, submit result, fills, and rejects.

10. Order lifecycle and reconciliation
    - Bolt-v3 needs submit, cancel, fill, reject, expire, position, balance, and restart behavior proven end to end.

11. Venue-specific live readiness
    - Each venue needs signing, submit, cancel, fills, fees, collateral, balances, and canary proof.
    - Polymarket CLOB V2 support cannot be considered production-ready from compile/local mapping alone.

12. Concurrency, panic, and process model
    - The system needs an empirical answer for how many strategies/markets run per process and what happens on panic/restart.

13. Strict CI and release gates
    - CI should block unsafe changes, but production readiness also needs exact-SHA green CI, artifact identity, canary evidence, and rollback proof.

## Recommended Next Move

Do not start by making one strategy trade harder.

Start with the evidence ledger as the accepted source of truth, then pick one blocker and create one narrow implementation issue.

The highest-risk first implementation slice is:

> bolt-v3 supervised live-trading transition: strategy construction/registration, order lifecycle boundary, runner loop, and reconciliation scope.

That slice should still be staged:

1. Build and register one real NT strategy from v3 TOML.
2. Keep it no-trade first.
3. Prove activation and instrument readiness.
4. Add decision-event persistence before submit.
5. Add dry-run/shadow mode.
6. Only then allow a tiny live canary.

## Python Wedge Position

Using Python to place real canary trades can be useful if it accelerates market learning.

But Python should be treated as a live-trading evidence generator, not hidden bolt-v3 architecture.

Acceptable use:

- Tiny controlled trades.
- One strategy.
- One venue.
- Full decision/order/fill/reject logs.
- Explicit kill switch.
- Lessons fed back into bolt-v3 requirements.

Not acceptable:

- Letting Python become the untracked production runtime.
- Duplicating bolt-v3 architecture in Python.
- Using Python success to claim Rust/NT production readiness.

## Practical Recommendation

Commit the evidence ledger and this summary together after review.

Then create exactly one issue for the first production blocker. The issue should include:

- The blocker name.
- The ledger rows it addresses.
- Accepted evidence.
- Missing proof.
- Files in scope.
- Files out of scope.
- Required verification.
- Explicit non-goals.

The next PR should close only that slice.
