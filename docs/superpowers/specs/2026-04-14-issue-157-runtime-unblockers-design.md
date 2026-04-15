# Issue 157 Runtime Unblockers Design

## Goal

Unblock honest implementation of `#135` without widening `#135` into runtime/config architecture.

This design covers only runtime/plumbing work already moved into `#157`:

- remove unconditional selector preemption on any open runtime position
- expose the configured `reference.publish_topic` to strategy construction
- surface NT periodic position checks through bolt config/startup

It does **not** implement strategy EV logic, entry/exit logic, or any `#135` behavior beyond the runtime seams `#135` needs.

## Problem

Two runtime-side blockers remain:

1. Selector preemption is too broad.
   The selector currently suppresses candidate loading whenever `cache.positions_open(None, None, Some(&strategy_id), None, None)` is non-empty. That prevents rotated 5m markets from being evaluated at all.

2. Strategies do not receive the configured reference publish topic.
   `ReferenceActor` already publishes `ReferenceSnapshot` on `cfg.reference.publish_topic`, but `StrategyBuildContext` only carries `fee_provider`. A strategy cannot honestly subscribe to `ReferenceSnapshot` without reopening runtime/config plumbing.

There is also a missing operational seam:

3. NT periodic position checks exist upstream but are not surfaced through bolt-v2 startup/config. This prevents runtime-side stale position cleanup work from being wired intentionally.

## Recommended Design

### 1. Expand `StrategyBuildContext`

Add a `reference_publish_topic: String` field to `StrategyBuildContext`.

This is the minimal runtime-owned seam needed by strategies. It keeps the source of truth in top-level config, but exposes the already-configured topic to strategy builders without making strategies parse runtime config directly.

### 2. Remove unconditional selector preemption

Stop short-circuiting the selector loop before candidate loading when open positions exist.

Instead:

- always load and evaluate current candidates while the node is running
- continue publishing `RuntimeSelectionSnapshot`
- leave any narrower “hold incumbent vs allow rotation” policy to explicit runtime/strategy logic built on top of the selection snapshot, rather than a hardcoded blanket stop

This removes the wrong architecture without inventing a new fixed-count policy in runtime.

### 3. Surface NT periodic position checks

Add a bolt runtime/config path for NT `position_check_interval_secs`.

The narrow builder-only approach is not available through the public NT API:

- upstream `LiveExecEngineConfig` already has `position_check_interval_secs`
- bolt-v2 currently starts through `LiveNode::builder(...)`
- the public `LiveNodeBuilder` does not expose a setter for `position_check_interval_secs`

So this slice requires a broader local startup rewrite:

- assemble `LiveNodeConfig` explicitly
- set `config.exec_engine.position_check_interval_secs`
- build the node through `LiveNode::build(name, Some(config))`
- locally reproduce the current client-registration path using the public kernel/data/execution APIs

This remains optional and fail-closed:

- default remains `None`
- when configured, bolt passes the value through to the NT live execution-engine config

This enables runtime-side cleanup without introducing strategy policy into strategy code.

## Data Flow

After this change:

1. `reference.publish_topic` is configured once in bolt config.
2. `ReferenceActor` publishes `ReferenceSnapshot` on that topic as it already does.
3. Startup wiring copies that topic into `StrategyBuildContext`.
4. Runtime-managed strategies can subscribe to:
   - `RuntimeSelectionSnapshot`
   - `ReferenceSnapshot`
   using supported seams rather than local branch drift.
5. Selector continues to publish candidate decisions even when positions are open.
6. Optional NT position checks run when configured.

## Files In Scope

- `src/strategies/registry.rs`
- `src/main.rs`
- `src/platform/runtime.rs`
- config/startup files needed to surface `position_check_interval_secs`
- runtime/config tests

## Non-Goals

- Strategy EV math
- Side selection
- Sizing
- Entry/exit behavior
- Broader multi-venue runtime policy
- Any attempt to finish `#135` on this branch

## Verification Targets

- Existing `platform_runtime` baseline remains green.
- New tests prove selector still loads/publishes while positions are open.
- New tests prove `StrategyBuildContext` carries `reference_publish_topic`.
- New tests prove bolt can pass `position_check_interval_secs` into NT startup config.
- New tests prove the rewritten startup path still registers clients and starts cleanly.

## Acceptance Boundary

`#157` is done when:

- `#135` no longer needs local drift to receive `ReferenceSnapshot`
- rotated 5m handling is no longer blocked by unconditional open-position preemption
- optional periodic NT position checks can be enabled intentionally through bolt config
