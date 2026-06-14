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

---

## Internal Review Follow-up Disposition

**Reviewer**: Codex internal adversarial review
**Date**: 2026-06-14
**Trigger**: Internal review against exact PR head `bc3ba9672a7fb7c3766da85660dc9fa5a9c5df0b` found remaining verifier coverage gaps.

### Findings Disposition

#### H3 - Strategy-policy rules did not cover future strategy modules

**Status**: Fixed in implementation follow-up

The strategy-policy fence now scans the registered strategy source roots plus every production Rust file under `src/strategies/**/*.rs`. A regression test creates a temporary future strategy module and proves `collect_violations()` catches strategy-local execution policy construction there.

#### M1 - Lowercase Strategy aliases escaped direct-mutation detection

**Status**: Fixed in implementation follow-up

The direct NT mutation qualifier regex now accepts ordinary Rust identifier/path qualifiers instead of only `Self` and UpperCamel aliases, and it recognizes turbofish-qualified mutation references. Regression coverage includes lowercase `Strategy as nt_strategy` calls and method references.

### Checks

- Red verification: the two new tests failed before the verifier change.
- Green verification: `python3 scripts/test_verify_bolt_v3_strategy_policy_fence.py` and `python3 scripts/verify_bolt_v3_strategy_policy_fence.py` passed after the fix.

---

## Final External Review Follow-up Disposition

**Reviewer**: External final-review synthesis
**Date**: 2026-06-14
**Trigger**: Final review against exact PR head `f26e09b6ea4c4fa5fb90903b7b9de0410b83fd4b` found command-transport and policy-alias fence gaps.

### Findings Disposition

#### H4 - NT OrderManager command transport bypass was not fenced

**Status**: Fixed in implementation follow-up

The direct NT mutation fence now covers raw `StrategyCore` / `OrderManager` command transport names: `core_mut`, `order_manager`, `send_risk_command`, `send_exec_command`, `send_emulator_command`, and `send_algo_command`. It also fences adjacent NT-managed lifecycle helpers that can transitively submit/cancel through NT core: GTD expiry helpers, market-exit finalization/cancel helpers, and deny-order helpers.

#### H5 - Strategy-local execution policy aliases and method pointers were not fenced

**Status**: Fixed in implementation follow-up

Strategy source now rejects any direct `BoltV3OrderExecutionPolicy` or `BoltV3OrderExecutionMode` type reference outside the shared policy module and `StrategyBuildContext` registry owner. This catches aliases, type aliases, constructor method pointers, and direct enum access instead of only literal constructor calls.

#### H6 - Future strategy source roots were not required to be source-integrity gated

**Status**: Fixed in implementation follow-up

The strategy policy fence now fails any production strategy source root under `src/strategies/*` that is not listed in `STRATEGY_SOURCE_ROOTS`. This forces future strategy roots under the same source-integrity/tamper-evidence registry before they can enter the tree.

### Checks

- Red verification: command transport, policy alias/method pointer, and ungated strategy-root tests failed before the verifier change.
- Green verification: `python3 scripts/test_verify_bolt_v3_strategy_policy_fence.py` now runs 18 tests and passes; `python3 scripts/verify_bolt_v3_strategy_policy_fence.py` passes at the branch head.

---

## Second Final External Review Follow-up Disposition

**Reviewer**: External adversarial re-review synthesis
**Date**: 2026-06-14
**Trigger**: Re-review against exact PR head `62550350f4d998e6905e6312353d6737536b2252` found the raw msgbus primitive below the fenced OrderManager wrappers and a policy-scope path heuristic gap.

### Findings Disposition

#### H7 - Raw msgbus trading-command injection primitive was not fenced

**Status**: Fixed in implementation follow-up

The prior H4 fix fenced `StrategyCore` / `OrderManager` wrapper names but did not fence the raw `nautilus_common::msgbus` trading-command primitive those wrappers call. The direct NT mutation fence now also covers `send_trading_command`, `send_any`, `send_any_value`, risk/exec/emulator/algo execute queue endpoint accessors, and the `TradingCommand` mutation surface names outside `src/bolt_v3_order_execution.rs`.

#### H8 - Execution-policy fabrication checks were strategy-path scoped

**Status**: Fixed in implementation follow-up

Execution-policy construction, type-reference, and override rules now run across production `src/**/*.rs`, not only registered strategy roots and `src/strategies/**`. Only the known policy/config/load/registration boundary files are allowlisted. The fence also rejects registry entries that try to register a builder through `crate::...` or `super::...` outside the `crate::strategies::...` module tree, so a registered strategy cannot bypass the strategy-tree/source-integrity assumptions by living under an arbitrary `src/foo.rs` path.

### Checks

- Red verification: raw msgbus command injection and outside-tree registry tests failed before the verifier change.
- CI plan: push this follow-up and use exact-head remote CI as the authoritative green check.
