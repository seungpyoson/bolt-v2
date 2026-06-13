# Internal Adversarial Review: Global Shadow Execution Policy

**Reviewer**: Codex self-review  
**Date**: 2026-06-13  
**Scope**:

- `specs/621-global-shadow-execution-policy/spec.md`
- `specs/621-global-shadow-execution-policy/plan.md`
- `specs/621-global-shadow-execution-policy/research.md`
- `specs/621-global-shadow-execution-policy/contracts/order-execution-policy.md`
- `specs/621-global-shadow-execution-policy/tasks.md`

## Findings

### B1 - Contract example used the wrong casing for existing `runtime.mode`

**Severity**: Blocking before external review  
**Status**: Fixed

The first contract draft showed:

```toml
[runtime]
mode = "live"
order_execution_mode = "shadow"
```

Current repo config uses `mode = "Live"` because `RuntimeBlock.mode` is typed as NautilusTrader `Environment`, and existing tests pin `Environment::Live`. Lowercase `live` in the contract could make reviewers think this feature redefines the existing environment field or adds a second mode grammar.

**Disposition**: Updated `contracts/order-execution-policy.md` to use `mode = "Live"` while keeping the new Bolt-owned field as `order_execution_mode = "shadow"`.

## Checks

- Template scan: no unresolved clarification markers or plan escape hatches in the new spec packet.
- Scope check: packet remains focused on globalizing PR #621 shadow/no-submit execution policy; no new venue, order variant, live-readiness, or adapter capability scope is claimed.
- Boundary check: plan keeps `bolt_v3_order_intent.rs` free of submit/admission/shadow policy and puts shared execution routing in a separate module.
- Safety check: managed NT venue-action knobs are rejected globally under shadow mode before strategy construction.
- Evidence check: shadow PnL compatibility remains explicit through admitted decision evidence without consuming live submit capacity.

## Verdict

VERDICT: APPROVE

The plan/spec packet is approved for external adversarial review. Implementation remains blocked until Gemini, Grok, and Claude also approve with no unresolved blockers.
