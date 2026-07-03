# Money Loop PR-B Explicit Governance Mode Design

Part of #1179.

## Goal

PR-B prevents live submit-capable strategies from booting with an execution client that is not covered by engaged exposure accounting unless the runtime config explicitly declares supervised deposit-capped mode.

## Scope

The boot invariant is per submit-capable execution client:

- Capital admission configured for submit enforcement covers an execution client when the client's venue and execution account match the capital admission config.
- Provider live-submit approval limits cover only the execution client id they are keyed to.
- An enabled loss-governor policy covers an execution client when the client's execution account matches the loss-governor account.
- If any submit-capable execution client is not covered by one of those controls, live boot must fail closed unless config declares supervised deposit-capped mode.

The declaration does not add a cap, threshold, order count, notional limit, timeout, or strategy gate. It is operator intent evidence only.

## Declaration

Use a typed TOML declaration in the risk config:

```toml
[risk.live_submit_governance]
mode = "supervised_deposit_capped"
```

`supervised_deposit_capped` means the operator is deliberately running without admission, provider approval limits, or an enabled loss-governor policy because the run is supervised and deposit-capped outside the node. It does not make the node safer by itself and must not be treated as a substitute for PR-A venue truth, PR-C exit clamping, or PR-D settlement booking.

## Runtime Behavior

Live-node construction checks the already-loaded strategy/client graph and controls before constructing submit admission:

- No strategies: allow the strategy-free path.
- Every submit-capable execution client covered by capital admission, a matching approval-limits entry, or matching loss policy: allow boot.
- Any uncovered submit-capable execution client plus `risk.live_submit_governance.mode = "supervised_deposit_capped"`: allow boot as explicitly declared.
- Any uncovered submit-capable execution client and no declaration: return a loud risk-policy build error before strategy registration naming the uncovered `execution_client_id`.

The per-submit admission evaluator remains unchanged for test/shadow paths; PR-B fixes live boot wiring so production cannot accidentally create an ungated submit admission state for a submit-capable strategy.

## Evidence

- Boot test: submit-capable config with no capital admission, no provider approval limits, no loss policy, and no declaration rejects before live-node build completes.
- Boot test: the same config with `risk.live_submit_governance.mode = "supervised_deposit_capped"` builds and still reports no capital admission or loss governor configured.
- Boot test: two submit-capable execution clients where one has an approval-limits entry and the other has no coverage rejects and names the uncovered client.
- Submit-admission differential test: no loss governor admits the request through remaining gates, while an enabled but unfed loss governor rejects the same request with `StaleLossSnapshot` and missing-snapshot halt evidence.
- Config/schema tests prove the tracked production profile carries the declaration through overlay composition.
