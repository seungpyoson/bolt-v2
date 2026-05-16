# Research: Thin Bolt-v3 Live Canary Path

## Problem Statement

The project now has Phases 3-5 merged on authoritative `main`, including the bolt-v3 production entrypoint, configured strategy registration, and mandatory decision evidence. Phase 6 must restart from that current main and add the missing submit admission gate before any live order reaches NautilusTrader submit.

## Hard Evidence

- Current source of truth is clean `main == origin/main == a5c60f2b6a4fe67fc80cf9d234f1512af09bec03`.
- Production enters bolt-v3: `src/main.rs:56-57` builds with `build_bolt_v3_live_node` and runs through `run_bolt_v3_live_node`.
- Bolt-v3 build maps TOML/SSM/adapters into NT client registration and strategy registration: `src/bolt_v3_live_node.rs:395-419`.
- Live canary gate exists before runner entry: `src/bolt_v3_live_node.rs:312-317`.
- Runtime capture is wired around the NT runner: `src/bolt_v3_live_node.rs:317-339`; Phase 6 must preserve this behavior.
- Gate report is validated but not retained for submit admission: `src/bolt_v3_live_node.rs:312-317`.
- Gate report fields are available for admission: `src/bolt_v3_live_canary_gate.rs:32-38`.
- Mandatory decision evidence is created once per registration pass and shared into strategy contexts: `src/bolt_v3_strategy_registration.rs:97-119`.
- The only direct strategy NT submit helper records decision evidence first: `src/strategies/binary_oracle_edge_taker.rs:2825-2834`.
- There is no `src/bolt_v3_submit_admission.rs` or `tests/bolt_v3_submit_admission.rs` on current main.
- PR #316 and #317 are stale stacked drafts based on pre-merge branches; they may inform design but must not be merged, rebased, or ported wholesale.
- NT-first doctrine says Bolt owns TOML, SSM policy, explicit runtime values, safe NT config conversion, and startup checks only when NT would fail poorly: `docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md:167-180`.
- Runtime contracts say NT owns account, position, order, fill, balance, average-price, and exposure state: `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:193-195`.
- Runtime contracts forbid a Bolt executable-order schema and venue translation layer: `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:676-688`.
- Runtime contracts state the live canary gate does not count orders or enforce per-order notional at submit time: `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md:1410-1418`.

## Decisions

### Decision 1: Phase 6 starts from authoritative main, not stale stacked branches

PR #316 and PR #317 should close as stale/superseded after user approval for GitHub mutation. Valid missing scope is re-planned from current `main`, with stale branch code treated only as forensic input.

### Decision 2: Canary uses final production path with tiny config

The canary is not a special runtime architecture. It is the final bolt-v3 production path with tiny TOML caps, operator approval, and extra artifact requirements.

### Decision 3: Entrypoint adoption is complete before Phase 6

`src/main.rs` now enters through the bolt-v3 build/run wrapper. Phase 6 must not reopen a direct production `node.run()` path.

### Decision 4: Submit admission is separate from the runner gate

The live canary runner gate blocks runner entry without approval/readiness. It does not enforce order count or notional at submit. A new submit-admission module must consume the validated gate report and sit before every NT submit call.

### Decision 5: Admission order is evidence, admission, NT submit

Decision evidence persistence must happen before admission consumes order budget. Submit admission then enforces order-count and per-order notional. NT `submit_order` is last.

### Decision 6: Concrete expansion belongs at edges

The MVP may start with Polymarket execution, Chainlink primary reference, and exchange reference venues, but core must accept additional providers/venues through provider bindings and TOML. More adapters are valuable only when added through this edge pattern.

### Decision 7: Backtesting is outside MVP

Backtesting and analytics should be specified after the live canary spine exists. Adding them before one live proof risks spending time on a research platform instead of proving the production runtime path.

## Rejected Approaches

- **Local mock venue worlds as live readiness**: rejected because they do not prove NT adapter authentication, user channel behavior, venue acceptance, or restart reconciliation.
- **Bolt-owned order lifecycle or reconciliation**: rejected because NT owns those surfaces.
- **Hardcoded Polymarket/Binance/Chainlink core path**: rejected because it violates generic core and prevents adapter expansion by config.
- **Verifier expansion for test-local literals**: rejected because it does not reduce live-trading risk.
- **External review before clean exact-head branch**: rejected by repo review bar.
- **Port PR #317 wholesale**: rejected because it is based on a stale stack and does not preserve current-main runtime-capture runner behavior.
