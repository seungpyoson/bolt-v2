# Issue 157 Runtime Unblockers Implementation Plan

> **Execution mode:** use `subagent-driven-development`.

## Goal

Implement the minimum runtime/plumbing changes needed so `#135` can stay strategy-only.

## Task 1: Strategy Build Context Reference Topic

**Files**
- `src/strategies/registry.rs`
- `src/main.rs`
- tests touching startup/build-context wiring

- [ ] Add `reference_publish_topic: String` to `StrategyBuildContext`.
- [ ] Thread `cfg.reference.publish_topic.clone()` into the runtime strategy build context in `main.rs`.
- [ ] Keep `fee_provider` behavior unchanged.
- [ ] Add focused tests proving the field is carried through the build context.
- [ ] Run targeted tests for registry/startup wiring.

## Task 2: Remove Unconditional Selector Preemption

**Files**
- `src/platform/runtime.rs`
- `tests/platform_runtime.rs`

- [ ] Remove the blanket `positions_open(...) => Vec::new()` selector short-circuit.
- [ ] Keep selector polling gated on `NodeState::Running`.
- [ ] Keep fail-closed behavior on loader/audit errors.
- [ ] Update/add tests so runtime still loads/publishes selection decisions while open positions exist.
- [ ] Preserve existing persistence/shutdown guarantees for the runtime-managed strategy.
- [ ] Run `cargo test --test platform_runtime`.

## Task 3: Surface NT Periodic Position Checks

**Files**
- startup/config files that define and render live runtime config
- tests covering parsing/render/startup wiring

- [ ] Add an optional bolt config field for NT `position_check_interval_secs`.
- [ ] Replace the current builder-only startup path where needed with explicit `LiveNodeConfig` assembly.
- [ ] Set `config.exec_engine.position_check_interval_secs` on that explicit config.
- [ ] Reproduce the current data/exec client registration behavior with the public kernel/data/execution APIs.
- [ ] Default remains unset (`None`).
- [ ] Add tests proving default `None` and configured passthrough.
- [ ] Add tests proving the rewritten startup path still builds and runs with registered clients.
- [ ] Run targeted config/startup tests.

## Task 4: Full Verification

- [ ] `cargo fmt --check`
- [ ] `cargo clippy -- -D warnings`
- [ ] `cargo test --test platform_runtime`
- [ ] targeted config/startup tests for the new position-check seam

## Review Lanes

- [ ] adversarial review on the exact candidate head
- [ ] verifier pass with command evidence
- [ ] code-quality review before final closeout
