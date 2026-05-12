# Research: Thin Bolt-v3 Live Canary Path

## Problem Statement

The project has a partially built bolt-v3 path, but current `main` still has a production legacy runner path and bolt-v3 does not yet prove a tiny live trade. The correct direction is not more local simulation. It is a thin NT-native path with one production entrypoint, one submit admission path, real no-submit readiness, and one tiny live canary.

## Hard Evidence

- Production still runs legacy path: `src/main.rs:240-242` directly creates `node.run()`.
- Bolt-v3 build exists but is no-submit/no-strategy: `src/bolt_v3_live_node.rs:1-35`.
- Bolt-v3 build maps TOML/SSM/adapters into NT client registration: `src/bolt_v3_live_node.rs:240-258`.
- PR #305 gate exists before runner entry: `src/bolt_v3_live_node.rs:268-277`.
- Gate report is not yet submit admission state: `src/bolt_v3_live_node.rs:272-276`.
- Provider bindings are registry-shaped but currently only Polymarket and Binance: `src/bolt_v3_providers/mod.rs:111-132`.
- Strategy archetype validation registry currently has one binding: `src/bolt_v3_archetypes/mod.rs:34-37`.
- NT-first doctrine says Bolt owns TOML, SSM policy, explicit runtime values, safe NT config conversion, and startup checks only when NT would fail poorly: `docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md:167-180`.
- Runtime contracts say NT owns account, position, order, fill, balance, average-price, and exposure state: `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:191-195`.
- Runtime contracts forbid a Bolt executable-order schema and venue translation layer: `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:676-688`.
- Runtime contracts state the live canary gate does not count orders or enforce per-order notional at submit time: `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:1410-1418`.

## Decisions

### Decision 1: Canary uses final production path with tiny config

The canary is not a special runtime architecture. It is the final bolt-v3 production path with tiny TOML caps, operator approval, and extra artifact requirements.

### Decision 2: Entrypoint adoption precedes live proof

No authenticated canary proves production readiness while `src/main.rs` can still bypass bolt-v3. The first runtime code slice must remove the legacy production run path or make it unreachable.

### Decision 3: Submit admission is separate from PR #305 runner gate

PR #305 blocks runner entry without approval/readiness. It does not enforce order count or notional at submit. A new submit-admission module must consume the validated gate report and sit before every NT submit call.

### Decision 4: Concrete expansion belongs at edges

The MVP may start with Polymarket execution, Chainlink primary reference, and exchange reference venues, but core must accept additional providers/venues through provider bindings and TOML. More adapters are valuable only when added through this edge pattern.

### Decision 5: Backtesting is outside MVP

Backtesting and analytics should be specified after the live canary spine exists. Adding them before one live proof risks spending time on a research platform instead of proving the production runtime path.

## Rejected Approaches

- **Local mock venue worlds as live readiness**: rejected because they do not prove NT adapter authentication, user channel behavior, venue acceptance, or restart reconciliation.
- **Bolt-owned order lifecycle or reconciliation**: rejected because NT owns those surfaces.
- **Hardcoded Polymarket/Binance/Chainlink core path**: rejected because it violates generic core and prevents adapter expansion by config.
- **Verifier expansion for test-local literals**: rejected because it does not reduce live-trading risk.
- **External review before clean exact-head branch**: rejected by repo review bar.
