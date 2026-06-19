# Loss Halt Market Exit Boundary

Historical note: this plan predates the #738 durable kill-switch consolidation.
#738 intentionally keeps active market exit unwired and rejects direct NT market-exit / live-flatten paths until the shared execution-policy path is designed.
Treat the original wording below as a historical boundary note for the older loss-halt slice, not operator guidance for #738.

## Objective

Do not add Bolt-owned market-exit policy, config, latch, or dispatch scaffolding. If active loss-halt market exit is needed, call NautilusTrader's owned `Trader::market_exit_strategy` primitive directly from a real live boundary.

## Scope

- Keep `[risk.loss_governor]` scoped to explicit trading-state action and manual recovery mode fields.
- Evaluate loss halt decisions from the existing shared loss-governor policy and NT-derived snapshots.
- Set the configured NT risk trading state after trusted loss snapshots.
- Leave active market exit out of this slice.
- Keep strategies as intent-only components; no strategy file should implement submit mechanics or loss-halt exit construction.

## Non-Scope

- No Bolt-built cancel order, flatten order construction, market-exit policy, or market-exit latch.
- No claim that NT `Reducing` proves the account is flat.
- No external operator clear-to-Active command surface.
- No durable operator authorization/audit implementation for manual recovery.
- No maker quote-set simultaneous-fill reservation model.
- No replace-submit reservation transition model.
- No adapter/venue collateral spendability or allowance proof.
- No dynamic market metadata or non-binary product calculators.
- No live reconnect/runtime proof beyond focused tests.

## Implementation Notes

- `src/bolt_v3_loss_halt_actions.rs` owns the pure trading-state decision and manual recovery evidence structure.
- `src/bolt_v3_validate.rs` owns config/action compatibility.
- `src/bolt_v3_loss_runtime_feed.rs` invokes the configured action handler only after a complete loss snapshot is published.
- `src/bolt_v3_live_node.rs` owns NT risk-state side effects for loss halts and manual recovery.

## Verification Targets

- Config parsing rejects missing enabled action fields.
- Dead gate/proof source-fence remains clean.
