# Production Kill Switch Research

## Current Repo State

- Current branch while researching: `codex/nt-loss-governor-circuit-breaker`.
- PR #480 was verified open, draft, merge-clean, and green on head `25d0e32bdaf09eac52621d2d9eda6f6703aac1a7`.
- The accepted loss-governor slice explicitly stops at submit admission and excludes cancel/flatten side effects. See `specs/505-nt-loss-governor/spec.md` FR-012 and assumptions.
- The roadmap status map already classifies the broader execution gate / kill switch as missing work beyond the canary submit gate.

## Existing Bolt Surfaces

### Submit Admission

- `src/bolt_v3_submit_admission.rs` owns the shared submit-admission state.
- `BoltV3SubmitAdmissionState` holds a `Mutex` with gate report, order counters, optional loss-governor policy, and latest loss snapshot.
- `admit_at` records admission evidence before either incrementing counters or returning a rejection.
- Current loss-governor handling is admission-only: it exempts `RiskReducingExit`, evaluates entries/replaces against `evaluate_loss_admission`, and returns `RejectedLossGovernorHalted` with halt reasons.
- Existing strategy submit calls flow through `submit_order_with_decision_evidence`, which records order intent, calls submit admission, then calls NT `submit_order`.

Implication: the global kill switch should extend this shared admission boundary with a durable halt latch. It should not place kill logic in individual strategies.

### Loss-Governor Runtime Feed

- `src/bolt_v3_live_node.rs` wires loss-governor runtime handlers only when `[risk.loss_governor]` is enabled.
- It subscribes to NT position events and portfolio snapshots and updates the shared submit-admission state with a `LossSnapshot`.
- The feed intentionally derives facts from NT messages, not venue-local balances.

Implication: loss-governor breach should become one kill-switch trigger, but cancel, flatten, and reconciliation need a separate global action supervisor.

### Runtime Capture And Observability

- `docs/bolt-v3/research/runtime-capture/nt-msgbus-surfaces.yaml` records that Bolt already captures:
  - `events.order.*` as `OrderEventAny`
  - `events.position.*` as `PositionEvent`
  - `events.portfolio.*` as `PortfolioSnapshot`
  - `events.risk` as `TradingStateChanged`
- The same document says `subscribe_positions` has no publisher on this pinned NT revision and should not be used as a proof surface.

Implication: reconciliation should query NT cache/portfolio state directly and use captured events as evidence, rather than relying on `positions.snapshots.*`.

## Pinned NautilusTrader Surfaces

Pinned NT revision from `Cargo.toml`: `6e059dcbb59ac1e582132fc431a581936c216c3c`.

Local checkout: `/Users/spson/.cargo/git/checkouts/nautilus_trader-3c6af4345b4d438b/6e059dc`.

### RiskEngine Trading State

- `crates/risk/src/engine/mod.rs` exposes `RiskEngine::set_trading_state(&mut self, TradingState)`.
- The method updates state and publishes `TradingStateChanged` on `events.risk`.
- `TradingState::Halted` denies submit orders and order lists.
- `TradingState::Reducing` denies submits that increase current exposure but allows reducing orders.
- `TradingState::Halted` also rejects order modification.

Implication: the design should set NT risk to `Reducing` immediately after a kill latch when flatten submits still need to pass, then use local/durable admission to block all new risk. A final `Halted` state can be considered after flat proof or for manual-intervention parking, but not before required flatten orders.

### Access From Bolt Runtime

- `LiveNode::kernel()` and `LiveNode::kernel_mut()` are public.
- `NautilusKernel` contains public `risk_engine`, `cache`, `portfolio`, data engine, and execution engine handles.
- `NautilusKernel::risk_engine()` returns `&Rc<RefCell<RiskEngine>>`.
- NT `TypedHandler` uses `Rc`, not `Arc`, and documents single-threaded message-bus use.

Implication: the global kill-switch runtime can be wired in `src/bolt_v3_live_node.rs` by cloning the relevant `Rc<RefCell<...>>` handles before `LiveNode::run`, instead of using cross-thread globals or command-endpoint hijacking.

### Order And Position State

- NT cache exposes `orders_open(...)`, `orders_open_count(...)`, `is_order_open(...)`, and pending-cancel/inflight helpers.
- NT cache exposes `positions_open(...)`, `positions_open_count(...)`, `is_position_open(...)`, and `has_positions_open(...)`.
- These methods filter by venue, instrument, strategy, account, and side.

Implication: cancel and reconciliation loops must enumerate more than visible open orders. They need config-owned filters over open, inflight, pending-cancel, emulated, algorithm-managed, contingent, and locally accepted-but-not-terminal order risk, plus open positions.

### Cancel And Flatten APIs

- NT `Strategy::cancel_order` builds `CancelOrder` from a cache-owned order and routes through emulation, exec algorithm, or execution engine as appropriate.
- NT `Strategy::cancel_orders` supports batch cancellation for a list of order IDs with validation.
- NT `Strategy::close_all_positions` enumerates open positions for instrument and strategy and submits reduce-only market orders using NT order construction.
- Existing Bolt strategy code already cancels pending entries and submits configured forced-flat exits, but that logic is strategy-local.

Implication: the global kill-switch design should not call bespoke venue APIs. It must also not assume one standalone kill-switch strategy can globally cancel or flatten other strategies through `Strategy::cancel_order` / `Strategy::close_all_positions`, because those helpers scope state to the calling strategy. The implementation issue should require a routing proof before side effects. Acceptable designs are:

1. a global supervisor with narrow per-registered-strategy action ports, where each strategy executes only its own NT cancel/flatten commands while policy, sequencing, evidence, and reconciliation stay global; or
2. a narrow live-node command router that snapshots NT cache state, preserves original strategy/client/order/position identity, initializes NT order/cache state correctly, and sends standard NT trading commands through the same risk/execution routes.

The first implementation should not choose either path blindly. It should add a source-grounded routing proof and tests before any cancel/flatten production code.

## Design Decisions

1. The kill switch is a global runtime supervisor with durable state, not an extension of one strategy.
2. The loss governor becomes one trigger source; it does not itself execute cancel/flatten.
3. Local admission latch and NT `TradingState::Reducing` work together: local latch blocks entries/replaces, NT risk blocks accidental exposure-increasing submits, and reducing exits remain possible.
4. Forced flattening cannot rely on ordinary `RiskReducingExit` admission alone because ordinary notional and live-order caps may be exhausted during a halt. It needs a distinct, proof-bound forced-reduction class.
5. Open-order cancellation and position flattening route through NT only.
6. Reconciliation proof uses NT cache/portfolio state plus captured event evidence. If mandatory proof is missing, the system remains halted.
7. Manual reset is an authorized, tamper-evident, evidence-producing operation; restart does not clear a halt.
8. PR #480-dependent live wiring should be documented as dependent until that PR lands on `main`.

## Open Implementation Questions

- Whether the final action boundary is a dedicated NT strategy/actor or an internal live-node runtime helper. The design recommends the dedicated actor first because it can use NT's public strategy methods.
- Whether the final action boundary is per-strategy action ports or an internal live-node command router. A standalone kill-switch strategy is not sufficient for global cancellation/flattening unless implementation proves it can act on every configured strategy's NT-owned orders and positions without strategy-id scoping bugs.
- Whether all configured accounts can be reconciled through one global supervisor or multiple account/instrument-family supervisors. The issue should require this to be explicit in implementation.
- Whether `TradingState::Halted` should be set automatically after `Flat`, or whether the durable local latch is sufficient until manual reset. The recommended default is `Reducing` during active action and `Halted` only after flat proof or failed manual-intervention parking.
- Which proof streams are mandatory versus optional for each deployment. Optional streams require stronger cache/portfolio query evidence and must be config-owned.
- How PR #480 reshapes the order-intent/admission boundary. Live submit/cancel/flatten wiring should be rebased after #480 lands.
