# Loss Halt Market Exit Slice

## Objective

Keep explicit loss-governor market-exit action config, but reject active direct NT market-exit actions until loss-halt exits can route through a Bolt-owned submit/cancel chokepoint.

## Scope

- Extend `[risk.loss_governor]` with explicit trading-state action, market-exit action, and manual recovery mode fields.
- Reject `market_exit = "all_registered_strategies"` because direct NT market-exit commands bypass Bolt submit/cancel chokepoints.
- Evaluate loss halt decisions from the existing shared loss-governor policy and NT-derived snapshots.
- Set the configured NT risk trading state after trusted loss snapshots.
- Keep reserved pure policy/latch structure for a future Bolt-owned exit path, without live dispatch in this slice.
- Keep strategies as intent-only components; no strategy file should implement submit mechanics or loss-halt exit construction.

## Non-Scope

- No Bolt-built cancel order or flatten order construction.
- No claim that NT `Reducing` proves the account is flat.
- No external operator clear-to-Active command surface.
- No durable operator authorization/audit implementation for manual recovery.
- No maker quote-set simultaneous-fill reservation model.
- No replace-submit reservation transition model.
- No adapter/venue collateral spendability or allowance proof.
- No dynamic market metadata or non-binary product calculators.
- No live reconnect/runtime proof beyond focused tests.

## Implementation Notes

- `src/bolt_v3_loss_halt_actions.rs` owns the pure action decision, manual recovery evidence structure, and reserved market-exit action/latch types.
- `src/bolt_v3_validate.rs` owns config/action compatibility and rejects active direct market-exit config.
- `src/bolt_v3_loss_runtime_feed.rs` invokes the configured action handler only after a complete loss snapshot is published.
- `src/bolt_v3_live_node.rs` owns NT risk-state side effects for loss halts and manual recovery.

## Verification Targets

- Config parsing rejects missing enabled action fields.
- Config parsing rejects `all_registered_strategies` market exit until a Bolt-owned exit path exists.
- Disabled loss-governor configs do not trigger market-exit loaded-strategy requirements.
- Dead gate/proof source-fence remains clean.
