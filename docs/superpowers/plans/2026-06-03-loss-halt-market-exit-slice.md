# Loss Halt Market Exit Slice

## Objective

Add a configured NT-owned market-exit action for loss-governor halts without reintroducing the removed live-canary gate/proof path and without claiming flat-position proof.

## Scope

- Extend `[risk.loss_governor]` with explicit trading-state action, market-exit action, and manual recovery mode fields.
- Require enabled market exit to use NT `TradingState::Reducing`; reject `Halted + market_exit` because `Halted` blocks strategy-control exit commands.
- Validate loaded strategy execution accounts match `[risk.loss_governor].account_id` when `market_exit = "all_registered_strategies"` is enabled.
- Evaluate loss halt decisions from the existing shared loss-governor policy and NT-derived snapshots.
- Set the configured NT risk trading state before dispatching active exits.
- Dispatch configured active loss exits through `Trader::market_exit_strategy` for registered strategies.
- Latch successful NT market-exit dispatches by `StrategyId`; retry failed dispatches on the next rejected snapshot and clear the latch only after observed NT `TradingState::Active`.
- Keep strategies as intent-only components; no strategy file should implement submit mechanics or loss-halt exit construction.

## Non-Scope

- No Bolt-built cancel order or flatten order construction.
- No claim that NT `Reducing` or `market_exit_strategy` proves the account is flat.
- No operator clear-to-Active live command surface.
- No durable operator authorization/audit implementation for manual recovery.
- No maker quote-set simultaneous-fill reservation model.
- No replace-submit reservation transition model.
- No adapter/venue collateral spendability or allowance proof.
- No dynamic market metadata or non-binary product calculators.
- No live reconnect/runtime proof beyond focused tests.

## Implementation Notes

- `src/bolt_v3_loss_halt_actions.rs` owns the pure action decision, manual recovery evidence structure, and market-exit latch.
- `src/bolt_v3_validate.rs` owns config/action compatibility and strategy account validation.
- `src/bolt_v3_loss_runtime_feed.rs` invokes the configured action handler only after a complete loss snapshot is published.
- `src/bolt_v3_live_node.rs` owns the NT side effects: `RiskEngine::set_trading_state` and `Trader::market_exit_strategy`.

## Verification Targets

- Config parsing rejects missing enabled action fields.
- Config parsing rejects `all_registered_strategies` market exit unless the matching trading-state action is `reducing`.
- Config parsing rejects active market exit if loaded strategy execution accounts differ from the loss-governor account.
- Disabled loss-governor configs do not trigger market-exit loaded-strategy requirements.
- NT market-exit dispatch reaches a running registered stub strategy hook.
- Dead gate/proof source-fence remains clean.
