# Money Loop PR-B Explicit Governance Mode Design

Part of #1179.

## Goal

PR-B prevents live submit-capable strategies from booting with no engaged exposure accounting unless the runtime config explicitly declares that ungoverned mode.

## Scope

The boot invariant is presence-based only:

- Capital admission configured for submit enforcement counts as an engaged control.
- Provider live-submit approval limits count as an engaged control.
- An enabled loss-governor policy counts as an engaged control.
- If submit-capable strategies are present and all three controls are absent, live boot must fail closed unless config declares the ungoverned mode.

The declaration does not add a cap, threshold, order count, notional limit, timeout, or strategy gate. It is operator intent evidence only.

## Declaration

Use a typed TOML declaration in the risk config:

```toml
[risk.live_submit_governance]
mode = "supervised_deposit_capped"
```

`supervised_deposit_capped` means the operator is deliberately running without admission, provider approval limits, or an enabled loss-governor policy because the run is supervised and deposit-capped outside the node. It does not make the node safer by itself and must not be treated as a substitute for PR-A venue truth, PR-C exit clamping, or PR-D settlement booking.

## Runtime Behavior

Live-node construction checks the already-loaded strategies and controls before constructing submit admission:

- No strategies: allow the strategy-free path.
- Any engaged control: allow boot.
- No engaged controls plus `risk.live_submit_governance.mode = "supervised_deposit_capped"`: allow boot as explicitly declared.
- No engaged controls and no declaration: return a loud risk-policy build error before strategy registration.

The per-submit admission evaluator remains unchanged for test/shadow paths; PR-B fixes live boot wiring so production cannot accidentally create an ungated submit admission state for a submit-capable strategy.

## Evidence

- Boot test: submit-capable config with no capital admission, no provider approval limits, no loss policy, and no declaration rejects before live-node build completes.
- Boot test: the same config with `risk.live_submit_governance.mode = "supervised_deposit_capped"` builds and still reports no capital admission or loss governor configured.
- Config/schema tests prove the tracked production profile carries the declaration through overlay composition.
