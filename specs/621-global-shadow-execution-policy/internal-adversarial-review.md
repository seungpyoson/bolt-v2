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

---

## Revision 2 Internal Review

**Reviewer**: Codex self-review
**Date**: 2026-06-13
**Trigger**: Claude external adversarial review returned `REQUEST_CHANGES` against the first external-review packet.

### External Blocking Findings Disposition

#### C1 - Shared routing covered submit/cancel only

**Status**: Fixed in revised packet

The first packet could let a future strategy call another NT mutation API directly. The revised contract now defines the full pinned NT strategy mutation surface: `submit_order`, `submit_order_list`, `modify_order`, `cancel_order`, `cancel_orders`, `cancel_all_orders`, `close_position`, and `close_all_positions`.

The revised architecture does not add speculative wrappers for unused NT methods. Instead, it implements submit/cancel for current production call sites and adds a source-fence/static verifier that rejects direct production-source calls to every listed method until a shared helper and live/shadow tests exist.

#### C2 - No source-fence enforces helper usage

**Status**: Fixed in revised packet

The revised spec, plan, data model, contract, and task list require a source-fence/static verifier. The verifier must fail production source that directly calls known NT venue mutation APIs outside `src/bolt_v3_order_execution.rs`.

#### C3 - NT managed-action reject list was asserted, not audited

**Status**: Fixed in revised packet

`research.md` now records the pinned NT `StrategyConfig` audit at rev `7c2aafb30fb143069c915a3f2057bb12174405f6`. It classifies independent mutation enablers, explains why identity, `oms_type`, market-exit tuning, and logging fields are not independent mutation enablers, and requires verifier failure when NT adds unclassified strategy config fields.

#### C4 - NT-initiated mutation scope was overclaimed

**Status**: Fixed in revised packet

The revised spec narrows the provable invariant to Bolt-strategy-originated NT venue mutations and loaded-strategy NT `StrategyConfig` managed-action knobs. It explicitly does not claim to firewall operator/manual exchange activity or adapter-level behavior outside loaded Bolt strategies.

### Checks

- Scope check: packet still globalizes PR #621's shadow/no-submit policy only; no new venue, order type, adapter, or live-readiness scope was added.
- Boundary check: `src/bolt_v3_order_execution.rs` remains the shared routing module; `src/bolt_v3_order_intent.rs` remains free of execution policy.
- Enforcement check: the revised plan makes the chokepoint enforceable with source-fence/static verification instead of convention.
- YAGNI check: unused NT mutation methods are fenced rather than wrapped speculatively.
- Evidence check: the NT config audit cites the pinned NT revision and classifies all current `StrategyConfig` fields.

### Verdict

VERDICT: APPROVE

The revised plan/spec packet is approved for a second external adversarial review pass. Implementation remains blocked until Gemini, Grok, and Claude approve the revised packet with no unresolved blockers.

---

## Final PR Review Disposition

**Reviewer**: External adversarial review synthesis
**Date**: 2026-06-14
**Trigger**: Final PR review against head `67343c036ea94e138ea0287797a774b900dade5f` returned additional structural fence findings.

### Blocking Findings Disposition

#### H1 - Public blanket raw NT mutation sink widened the bypass surface

**Status**: Fixed in implementation follow-up

The review found that the public `BoltV3NtVenueMutationSink` blanket implementation exposed `submit_order_via_nt(...)` and `cancel_order_via_nt(...)` as callable methods on every NT `Strategy`. The fix makes the raw mutation sink trait and NT strategy adapter module-private inside `src/bolt_v3_order_execution.rs`; public routing accepts a mutable NT `Strategy` reference and constructs the private adapter internally.

#### H2 - Direct NT mutation fence was qualifier- and root-specific

**Status**: Fixed in implementation follow-up

The fence now separates strategy-policy rules from a repo-wide production-source direct-mutation scan. Direct NT mutation calls are rejected outside `src/bolt_v3_order_execution.rs` across production `src/**/*.rs`, including method syntax, type-qualified/UFCS syntax, alias-qualified syntax, method-pointer references, private raw-adapter names, and near-neighbor mutation method variants. Strategy code is also fenced from constructing or overriding execution policy locally.

### Checks

- Boundary check: raw NT mutation helper methods are private to `src/bolt_v3_order_execution.rs`.
- Enforcement check: source-fence tests include `_via_nt`, alias-qualified, type-qualified, method-pointer, near-neighbor method, exact allowlist, and production-source scan coverage.
- Documentation check: contract and plan now describe the private-adapter implementation rather than a public closure/sink surface.

### Verdict

VERDICT: READY FOR FINAL REMOTE CI
